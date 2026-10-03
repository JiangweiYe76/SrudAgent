//! The ACP agent: JSON-RPC dispatch over the core runtime.
//!
//! [`Agent`] is the whole protocol surface. A host feeds it `rpc_request`
//! envelopes through [`Agent::handle`] and gets the JSON-RPC reply back, and
//! subscribes with [`Agent::subscribe`] for the `session/update` stream. It
//! owns no socket and no connection state beyond the `initialize` gate.
//!
//! Method coverage:
//!
//! | Method            | Behaviour                                            |
//! |-------------------|------------------------------------------------------|
//! | `initialize`      | version + capability negotiation (must come first)   |
//! | `session/new`     | register an in-memory session                        |
//! | `session/load`    | rebuild a session from its log and replay it         |
//! | `session/resume`  | rebuild a session from its log, without replaying    |
//! | `session/prompt`  | run one turn; the reply is the turn's end            |
//! | `session/cancel`  | interrupt the active turn                            |
//! | `session/list`    | snapshot live sessions                               |
//! | `session/close`   | interrupt + deregister                               |
//! | `session/delete`  | deregister                                           |
//! | `_srud/unstable/session/set_title` | rename a session               |
//!
//! Everything else — config/mode setters, `authenticate`, and the remaining
//! `_srud/unstable/*` extensions — answers `METHOD_NOT_FOUND`, matching what the
//! advertised capabilities promise.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use srud_core::client::ModelClient;
use srud_core::session::Session;
use srud_core::tools::ToolRegistry;
use srud_protocol::acp::methods::{
    INITIALIZE, SESSION_CANCEL, SESSION_CLOSE, SESSION_DELETE, SESSION_LIST, SESSION_LOAD,
    SESSION_NEW, SESSION_PROMPT, SESSION_RESUME, SRUD_SESSION_SET_TITLE,
};
use srud_protocol::acp::{
    AcpError, CancelNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock,
    DeleteSessionRequest, DeleteSessionResponse, Implementation, InitializeRequest,
    InitializeResponse, JsonRpcMessage, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, MaybeUndefined, NewSessionRequest, NewSessionResponse, Notification,
    PromptRequest, PromptResponse, ProtocolVersion, Request, RequestId, Response,
    ResumeSessionRequest, ResumeSessionResponse, SessionId, SessionInfoUpdate, SessionNotification,
    SessionUpdate, CLIENT_METHOD_NAMES,
};
use srud_protocol::error::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, SESSION_BUSY,
    SESSION_NOT_FOUND,
};
use srud_protocol::srud::methods::{SetSessionTitleRequest, SetSessionTitleResponse};
use srud_protocol::srud::turn_end::TurnEndWire;

use crate::convert::{prompt_outcome, prompt_text, replay_notification};
use crate::events::EventHub;
use crate::sessions::{RestoreError, SessionManager};

/// The JSON-RPC reply to one `rpc_request` envelope.
pub type RpcReply = JsonRpcMessage<Response<Value>>;

/// The agent-side success reply the host resolves its promise with.
#[must_use]
pub fn reply_ok(id: RequestId, result: Value) -> RpcReply {
    JsonRpcMessage::wrap(Response::new(id, Ok(result)))
}

/// The agent-side error reply.
#[must_use]
pub fn reply_err(id: RequestId, error: AcpError) -> RpcReply {
    JsonRpcMessage::wrap(Response::new(id, Err(error)))
}

/// An ACP v1 agent backed by the core runtime.
pub struct Agent {
    model: Arc<dyn ModelClient>,
    tools: Arc<ToolRegistry>,
    sessions: Arc<SessionManager>,
    hub: EventHub,
    initialized: AtomicBool,
}

impl Agent {
    /// Creates an agent over a model client and a tool registry.
    #[must_use]
    pub fn new(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            model,
            tools,
            sessions: Arc::new(SessionManager::new()),
            hub: EventHub::new(),
            initialized: AtomicBool::new(false),
        }
    }

    /// Subscribes to the outgoing `session/update` notifications.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Notification<SessionNotification>> {
        self.hub.subscribe()
    }

    /// Handles one JSON-RPC request and produces its reply.
    ///
    /// `session/prompt` resolves only when the turn ends — that is the
    /// protocol's completion signal. All other methods reply immediately.
    pub async fn handle(&self, request: JsonRpcMessage<Request<Value>>) -> RpcReply {
        let request = request.into_inner();
        let id = request.id;
        let params = request.params.unwrap_or(Value::Null);
        let method = request.method.as_ref();

        // `initialize` is the mandatory first request; everything else is
        // refused until the negotiation has happened.
        if method != INITIALIZE && !self.initialized.load(Ordering::Acquire) {
            return reply_err(
                id,
                AcpError::new(
                    INVALID_REQUEST,
                    "initialize must be the first request on the connection",
                ),
            );
        }

        let outcome = match method {
            INITIALIZE => self.handle_initialize(params).map(serialize),
            SESSION_NEW => self.handle_new_session(params).await.map(serialize),
            SESSION_LOAD => self.handle_load(params).await.map(serialize),
            SESSION_RESUME => self.handle_resume(params).await.map(serialize),
            SESSION_PROMPT => self.handle_prompt(params).await.map(serialize),
            SESSION_CANCEL => self
                .handle_cancel(params)
                .map(|()| Value::Object(Default::default())),
            SESSION_LIST => self.handle_list().await.map(serialize),
            SESSION_CLOSE => self
                .handle_close(params)
                .await
                .map(|()| serialize(CloseSessionResponse::new())),
            SESSION_DELETE => self
                .handle_delete(params)
                .await
                .map(|()| serialize(DeleteSessionResponse::new())),
            SRUD_SESSION_SET_TITLE => self.handle_set_title(params).await.map(serialize),
            // Known-but-unimplemented methods get the same treatment as
            // unknown ones: the capabilities never advertised them.
            _ => Err(AcpError::new(
                METHOD_NOT_FOUND,
                format!("method `{method}` is not supported"),
            )),
        };

        match outcome {
            Ok(result) => reply_ok(id, result),
            Err(error) => reply_err(id, error),
        }
    }

    fn handle_initialize(&self, params: Value) -> Result<InitializeResponse, AcpError> {
        let request: InitializeRequest = parse_params(params)?;
        if request.protocol_version != ProtocolVersion::V1 {
            return Err(AcpError::new(
                INVALID_PARAMS,
                format!(
                    "unsupported protocol version {}; this agent speaks v1 only",
                    request.protocol_version
                ),
            ));
        }
        self.initialized.store(true, Ordering::Release);
        Ok(InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(srud_protocol::acp::capabilities::agent_capabilities())
            .agent_info(Implementation::new("srud-agent", env!("CARGO_PKG_VERSION")))
            .auth_methods(vec![]))
    }

    async fn handle_new_session(&self, params: Value) -> Result<NewSessionResponse, AcpError> {
        let request: NewSessionRequest = parse_params(params)?;
        let session_id = self
            .sessions
            .create(Some(request.cwd))
            .await
            .map_err(|err| {
                AcpError::new(
                    INTERNAL_ERROR,
                    format!("could not create the session workspace: {err}"),
                )
            })?;
        Ok(NewSessionResponse::new(session_id))
    }

    async fn handle_prompt(&self, params: Value) -> Result<PromptResponse, AcpError> {
        let request: PromptRequest = parse_params(params)?;

        // The session and its log together, so a turn cannot be recorded against
        // another session's log. One lookup rather than two: the pair is
        // registered together and is never separated.
        let (session, log) = self
            .sessions
            .with_log(&request.session_id)
            .ok_or_else(|| session_not_found(&request.session_id))?;

        let text = prompt_with_validation(&request.prompt)?;
        self.name_from_first_prompt(&request.session_id, &text)
            .await;

        let sink = self.hub.sink_for(session.id());
        let result = srud_core::turn::run_turn(
            &session,
            srud_core::types::TurnInput { text },
            self.model.as_ref(),
            self.tools.as_ref(),
            log.as_ref(),
            &sink,
        )
        .await
        .map_err(|err| match err {
            srud_core::TurnError::Busy => {
                AcpError::new(SESSION_BUSY, "session already has an active turn")
            }
            // Distinct from `Busy`: nothing ran, and the session is still usable —
            // the log is what refused. Saying "busy" would tell the client to wait
            // for something that will never start.
            srud_core::TurnError::Unrecordable(err) => {
                AcpError::new(INTERNAL_ERROR, format!("cannot record this turn: {err}"))
            }
        })?;

        let wire: TurnEndWire = prompt_outcome(result.reason)?;
        let response = PromptResponse::new(wire.stop_reason);
        Ok(match wire.meta {
            Some(meta) => response.meta(meta.to_meta()),
            None => response,
        })
    }

    fn handle_cancel(&self, params: Value) -> Result<(), AcpError> {
        let request: CancelNotification = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        // Cooperative: this signals the turn's token; the prompt reply then
        // carries `stopReason: "cancelled"`.
        session.interrupt();
        Ok(())
    }

    async fn handle_list(&self) -> Result<ListSessionsResponse, AcpError> {
        Ok(ListSessionsResponse::new(self.sessions.list().await))
    }

    async fn handle_close(&self, params: Value) -> Result<(), AcpError> {
        let request: CloseSessionRequest = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        session.interrupt();
        // Closing leaves the log where it is: the session is over, not deleted, and
        // is what the next `session/list` reports so a client can load it again.
        self.sessions.forget(&request.session_id);
        Ok(())
    }

    async fn handle_delete(&self, params: Value) -> Result<(), AcpError> {
        let request: DeleteSessionRequest = parse_params(params)?;
        if !self.sessions.remove(&request.session_id).await {
            return Err(session_not_found(&request.session_id));
        }
        Ok(())
    }

    async fn handle_resume(&self, params: Value) -> Result<ResumeSessionResponse, AcpError> {
        let request: ResumeSessionRequest = parse_params(params)?;
        let loaded = self.restore(&request.session_id, &request.cwd).await?;
        // No replay: a client asking to resume is saying it already has the
        // conversation.
        self.broadcast_title(&request.session_id, loaded.title.clone());
        Ok(ResumeSessionResponse::new())
    }

    async fn handle_load(&self, params: Value) -> Result<LoadSessionResponse, AcpError> {
        let request: LoadSessionRequest = parse_params(params)?;
        let loaded = self.restore(&request.session_id, &request.cwd).await?;

        // Replayed before the response, because ACP requires the client to have
        // the whole conversation by the time `session/load` resolves. Anything
        // sent after would arrive as a change to a session the client already
        // believes it has.
        let reasons: std::collections::HashMap<_, _> = loaded.turn_ends.iter().cloned().collect();
        let mut replay = loaded.conversation.iter().peekable();
        while let Some(event) = replay.next() {
            // A turn is over once the next record belongs to another one, so its
            // reason goes on the last update of it — which is the only place a
            // replayed turn can be closed from.
            let last_of_its_turn = replay
                .peek()
                .is_none_or(|next| next.turn_id() != event.turn_id());
            let turn_end = if last_of_its_turn {
                event.turn_id().and_then(|turn| reasons.get(&turn).copied())
            } else {
                None
            };
            if let Some(notification) = replay_notification(&request.session_id, event, turn_end) {
                self.hub.send(notification);
            }
        }
        self.broadcast_title(&request.session_id, loaded.title.clone());
        Ok(LoadSessionResponse::new())
    }

    /// Brings a session back, or says why it could not be.
    async fn restore(
        &self,
        id: &SessionId,
        cwd: &std::path::Path,
    ) -> Result<srud_core::session_store::Loaded, AcpError> {
        self.sessions.restore(id, cwd).await.map_err(restore_failed)
    }

    /// Names an unnamed session after the message that opened it.
    ///
    /// A session is named by its first prompt and never re-derived: later turns
    /// must not overwrite it, and a rename the user has already made wins. When
    /// this call is the one that named the session, it broadcasts the title so
    /// every attached client renders it without asking.
    ///
    /// A title that cannot be recorded is not announced, but does not fail the
    /// prompt: the name is cosmetic, and the turn it opened still has to run. The
    /// failure is dropped, which is the one place that is the right answer — a
    /// disk that refused the title will refuse the turn's own records a moment
    /// later and report it there.
    async fn name_from_first_prompt(&self, session_id: &SessionId, text: &str) {
        let title = title_from_prompt(text);
        if let Ok(true) = self.sessions.name_if_unnamed(session_id, &title).await {
            self.broadcast_title(session_id, Some(title));
        }
    }

    async fn handle_set_title(&self, params: Value) -> Result<SetSessionTitleResponse, AcpError> {
        let request: SetSessionTitleRequest = parse_params(params)?;
        match self
            .sessions
            .set_title(&request.session_id, &request.title)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(session_not_found(&request.session_id)),
            Err(err) => {
                return Err(AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot record the title: {err}"),
                ));
            }
        }
        // Echo the effective title rather than the request's, so a blank title
        // is reported as the clear it was.
        let title = self.sessions.title(&request.session_id);
        self.broadcast_title(&request.session_id, title.clone());
        Ok(SetSessionTitleResponse { title, meta: None })
    }

    /// Publishes a title change on the `session/update` stream.
    ///
    /// ACP routes this through the standard progress channel as a
    /// `SessionInfoUpdate` variant rather than a method of its own, so a client
    /// already following `session/update` sees renames for free.
    fn broadcast_title(&self, session_id: &SessionId, title: Option<String>) {
        // The builder takes `IntoMaybeUndefined`, so a `String` becomes a set
        // title and `MaybeUndefined::Null` clears it — how ACP spells "no title".
        let title: MaybeUndefined<String> = match title {
            Some(title) => MaybeUndefined::Value(title),
            None => MaybeUndefined::Null,
        };
        let update = SessionInfoUpdate::new().title(title);
        self.hub.send(Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::SessionInfoUpdate(update),
            )),
        });
    }

    fn require_session(&self, id: &SessionId) -> Result<Arc<Session>, AcpError> {
        self.sessions.get(id).ok_or_else(|| session_not_found(id))
    }
}

/// Derives a session title from the message that opened it.
///
/// The message is a whole paragraph, so it is flattened to one line and cut to
/// a length a sidebar row can show. Truncation is on a character boundary, not
/// a byte one, so multi-byte text cannot panic.
fn title_from_prompt(text: &str) -> String {
    const MAX_CHARS: usize = 60;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_CHARS).collect();
    // Drop a trailing partial word so the title does not end mid-word.
    match cut.rfind(' ') {
        Some(idx) if idx > MAX_CHARS / 2 => cut[..idx].to_string(),
        _ => cut,
    }
}

fn session_not_found(id: &SessionId) -> AcpError {
    AcpError::new(
        SESSION_NOT_FOUND,
        format!("no session with id `{}`", id.0.as_ref()),
    )
}

/// What a client is told when a session could not be brought back.
fn restore_failed(err: RestoreError) -> AcpError {
    match &err {
        RestoreError::NotFound(id) => session_not_found(id),
        // The caller named a different directory, which ACP requires to match:
        // that is a bad request, not a server fault.
        RestoreError::Cwd { .. } => AcpError::new(INVALID_PARAMS, err.to_string()),
        RestoreError::Config(_) | RestoreError::Load(_) | RestoreError::Repair(_) => {
            AcpError::new(INTERNAL_ERROR, format!("cannot load the session: {err}"))
        }
    }
}

/// Deserialises method params, reporting invalid payloads as
/// `INVALID_PARAMS` with the serde message attached.
fn parse_params<T>(params: Value) -> Result<T, AcpError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(params)
        .map_err(|err| AcpError::new(INVALID_PARAMS, format!("invalid method params: {err}")))
}

fn serialize<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value)
        .unwrap_or_else(|err| Value::String(format!("response serialization failed: {err}")))
}

/// A prompt block that the declared capabilities do not accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedContent {
    /// An image block, refused while `promptCapabilities.image` is false.
    Image,
    /// An audio block, refused while `promptCapabilities.audio` is false.
    Audio,
    /// Any other non-text block (resources, links, tool references).
    Other(&'static str),
}

/// Validates a prompt against the declared text-only capabilities.
///
/// Returns the concatenated text, or an error naming the first block the
/// agent cannot accept — dropping multimodal input silently would make the
/// model answer a prompt the user never sent.
pub fn prompt_with_validation(blocks: &[ContentBlock]) -> Result<String, AcpError> {
    for block in blocks {
        let kind = match block {
            ContentBlock::Text(_) => continue,
            ContentBlock::Image(_) => UnsupportedContent::Image,
            ContentBlock::Audio(_) => UnsupportedContent::Audio,
            ContentBlock::Resource(_) => UnsupportedContent::Other("resource"),
            ContentBlock::ResourceLink(_) => UnsupportedContent::Other("resource_link"),
            _ => UnsupportedContent::Other("unknown"),
        };
        return Err(AcpError::new(
            INVALID_PARAMS,
            format!("this agent accepts text prompts only, got {kind:?}"),
        ));
    }
    Ok(prompt_text(blocks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{stream, StreamExt};
    use serde_json::json;
    use srud_core::client::{ModelEvent, ModelRequest, ModelStream};
    use srud_protocol::acp::methods::{
        SESSION_LIST, SRUD_SESSION_FORK, SRUD_SESSION_ROLLOUT_READ, SRUD_SESSION_SET_TITLE,
        SRUD_SESSION_STEER,
    };
    use srud_protocol::acp::TextContent;
    use srud_protocol::acp::AGENT_METHOD_NAMES;
    use srud_protocol::transport::tauri::RpcRequest;
    use tokio_util::sync::CancellationToken;

    /// The method names that answer `METHOD_NOT_FOUND`, so tests can assert
    /// the coverage boundary.
    const UNIMPLEMENTED_METHODS: &[&str] = &[
        AGENT_METHOD_NAMES.session_set_mode,
        AGENT_METHOD_NAMES.session_set_config_option,
        AGENT_METHOD_NAMES.authenticate,
        SRUD_SESSION_FORK,
        SRUD_SESSION_STEER,
        SRUD_SESSION_ROLLOUT_READ,
    ];

    /// Prompts a session, drains its turn, and returns the broadcast titles.
    ///
    /// The subscriber is attached before the prompt so the title broadcast on
    /// the first turn cannot be missed.
    async fn prompt_titles(
        agent: &Agent,
        session_id: &SessionId,
        text: &str,
    ) -> Vec<Option<String>> {
        let mut rx = agent.subscribe();
        agent
            .handle(request(
                SESSION_PROMPT,
                json!({
                    "sessionId": session_id.0.as_ref(),
                    "prompt": [{ "type": "text", "text": text }]
                }),
            ))
            .await;
        let mut titles = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            if value["params"]["sessionId"] != serde_json::json!(session_id.0.as_ref()) {
                continue;
            }
            if value["params"]["update"]["sessionUpdate"]
                == serde_json::json!("session_info_update")
            {
                titles.push(
                    value["params"]["update"]["title"]
                        .as_str()
                        .map(ToString::to_string),
                );
            }
        }
        titles
    }

    struct Scripted {
        events: Vec<ModelEvent>,
        /// When set, the stream stalls until the turn's cancellation token
        /// fires, then yields one more delta so the loop notices.
        stall_until_cancel: bool,
    }

    #[async_trait::async_trait]
    impl ModelClient for Scripted {
        async fn stream(
            &self,
            _request: ModelRequest,
            cancel: CancellationToken,
        ) -> Result<ModelStream, srud_core::client::ModelError> {
            let head: ModelStream = Box::pin(stream::iter(self.events.clone().into_iter().map(Ok)));
            let tail: ModelStream = if self.stall_until_cancel {
                // Stall until the turn is cancelled, emit one more delta to
                // wake the loop, then end. The loop's next cancellation check
                // turns this into `Interrupted`.
                Box::pin(stream::unfold(Some(cancel), |state| async move {
                    let token = state?;
                    token.cancelled().await;
                    Some((
                        Ok(ModelEvent::TextDelta {
                            delta: "after-cancel".into(),
                        }),
                        None,
                    ))
                }))
            } else {
                Box::pin(stream::iter(std::iter::once(Ok(ModelEvent::Done))))
            };
            Ok(Box::pin(head.chain(tail)))
        }
    }

    fn scripted(events: Vec<ModelEvent>) -> Arc<dyn ModelClient> {
        Arc::new(Scripted {
            events,
            stall_until_cancel: false,
        })
    }

    fn stalling(events: Vec<ModelEvent>) -> Arc<dyn ModelClient> {
        Arc::new(Scripted {
            events,
            stall_until_cancel: true,
        })
    }

    fn agent(model: Arc<dyn ModelClient>) -> Agent {
        Agent::new(model, Arc::new(ToolRegistry::new()))
    }

    fn request(method: &str, params: Value) -> RpcRequest {
        JsonRpcMessage::wrap(Request {
            id: RequestId::Number(1),
            method: method.into(),
            params: Some(params),
        })
    }

    async fn initialize(agent: &Agent) {
        let reply = agent
            .handle(request(
                INITIALIZE,
                json!({ "protocolVersion": 1, "clientCapabilities": {} }),
            ))
            .await;
        assert!(
            matches!(reply.inner(), Response::Result { .. }),
            "initialize should succeed: {reply:?}"
        );
    }

    async fn new_session(agent: &Agent) -> SessionId {
        // The workspace is settled from the configuration directory, which is
        // read from the environment. Holding the environment and pointing it at
        // a directory of this test's own keeps the session off the developer's
        // home and off whatever another test left behind.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-agent"),
        );
        new_session_in(agent, "").await
    }

    /// Creates a session working in `cwd`, or with a workspace of its own when
    /// `cwd` is empty.
    ///
    /// Without the environment setup `new_session` does, for a test that has to
    /// keep one configuration directory alive across two agents.
    async fn new_session_in(agent: &Agent, cwd: &str) -> SessionId {
        let reply = agent
            .handle(request(
                SESSION_NEW,
                // No working directory named, so the session gets a workspace of
                // its own. Spelled out rather than left to a path that may or may
                // not exist on the machine running the test.
                json!({ "cwd": cwd, "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new should succeed");
        };
        SessionId::new(result["sessionId"].as_str().unwrap().to_string())
    }

    /// Every `session/update` sent for a session, as `(kind, text)`.
    ///
    /// `session_info_update` is left out: it carries the title rather than the
    /// conversation, and it travels on the same channel.
    fn conversation(
        rx: &mut tokio::sync::broadcast::Receiver<Notification<SessionNotification>>,
    ) -> Vec<(String, String)> {
        let mut updates = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            let update = &value["params"]["update"];
            let Some(kind) = update["sessionUpdate"].as_str() else {
                continue;
            };
            if kind == "session_info_update" {
                continue;
            }
            updates.push((
                kind.to_string(),
                update["content"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        updates
    }

    /// Every `session/update` sent for a session, as `(turn id, turn end)`.
    ///
    /// The title update is left out, as in [`conversation`]: it belongs to the
    /// session rather than to a turn.
    fn replayed_turns(
        rx: &mut tokio::sync::broadcast::Receiver<Notification<SessionNotification>>,
    ) -> Vec<(String, Option<String>)> {
        let mut seen = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            if value["params"]["update"]["sessionUpdate"] == json!("session_info_update") {
                continue;
            }
            let srud = &value["params"]["_meta"]["srud"];
            seen.push((
                srud["turnId"].as_str().unwrap_or_default().to_string(),
                srud["turnEndReason"].as_str().map(str::to_string),
            ));
        }
        seen
    }

    fn result_of(reply: RpcReply) -> Value {
        match reply.into_inner() {
            Response::Result { result, .. } => result,
            Response::Error { error, .. } => panic!("expected success, got {error:?}"),
        }
    }

    fn error_of(reply: RpcReply) -> AcpError {
        match reply.into_inner() {
            Response::Error { error, .. } => error,
            Response::Result { result, .. } => panic!("expected error, got {result:?}"),
        }
    }

    #[tokio::test]
    async fn requests_before_initialize_are_refused() {
        let agent = agent(scripted(vec![]));
        let error = error_of(
            agent
                .handle(request(SESSION_NEW, json!({ "cwd": "/" })))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_REQUEST);
    }

    #[tokio::test]
    async fn initialize_negotiates_v1_and_capabilities() {
        let agent = agent(scripted(vec![]));
        let result = result_of(
            agent
                .handle(request(
                    INITIALIZE,
                    json!({ "protocolVersion": 1, "clientCapabilities": {} }),
                ))
                .await,
        );
        assert_eq!(result["protocolVersion"], json!(1));
        assert_eq!(result["agentCapabilities"]["loadSession"], json!(true));
        assert!(result["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
        assert!(result["agentCapabilities"]["sessionCapabilities"]["resume"].is_object());
        assert!(result.get("authMethods").is_some());
    }

    /// A session and the directory it works in, on a configuration directory of
    /// this test's own.
    ///
    /// Returned together because a load has to name the directory ACP will check
    /// against, and the caller has to keep the guard alive for it to be found.
    async fn session_with_a_home(agent: &Agent) -> (SessionId, String) {
        let cwd = std::env::temp_dir().join(format!("srud-load-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("a directory to work in");
        let cwd = cwd.to_string_lossy().into_owned();
        let id = new_session_in(agent, &cwd).await;
        (id, cwd)
    }

    #[tokio::test]
    async fn a_session_loads_back_from_its_log() {
        // The point of the log: an agent that restarted has nothing in memory and
        // everything on disk.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        // A second agent, as after a restart.
        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx),
            vec![
                ("user_message_chunk".to_string(), "hello".to_string()),
                ("agent_message_chunk".to_string(), "hi there".to_string()),
            ],
            "the conversation is replayed in order"
        );
    }

    #[tokio::test]
    async fn a_loaded_session_continues_where_it_left_off() {
        // Loading is not just replay: the agent has to have the history too, or
        // the next prompt answers as though the session were new.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-continue"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "first answer".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "first question").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        let session = second
            .sessions
            .get(&session_id)
            .expect("the session is live");
        assert_eq!(
            session.state().history(),
            &[
                // The env-context block the first turn recorded: it is part of
                // what the model saw, so it comes back with it.
                srud_core::context::item(std::path::Path::new(&cwd)),
                srud_core::types::ResponseItem::user("first question"),
                srud_core::types::ResponseItem::assistant("first answer"),
            ]
        );
    }

    #[tokio::test]
    async fn resuming_does_not_replay_the_conversation() {
        // What a client asks for by resuming: it already has the transcript.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-resume"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_RESUME,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert!(conversation(&mut rx).is_empty(), "nothing is replayed");
        assert!(second.sessions.get(&session_id).is_some(), "and it is live");
    }

    #[tokio::test]
    async fn a_live_session_loads_for_another_client() {
        // One client has the session open and another asks for it: the
        // conversation comes off the log, and nothing is invented for a session
        // that never stopped.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-live"),
        );

        let agent = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&agent).await;
        let (session_id, cwd) = session_with_a_home(&agent).await;
        prompt_titles(&agent, &session_id, "hello").await;

        let mut rx = agent.subscribe();
        result_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx),
            vec![
                ("user_message_chunk".to_string(), "hello".to_string()),
                ("agent_message_chunk".to_string(), "hi there".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn loading_a_session_that_never_existed_is_not_found() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-missing"),
        );

        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": "00000000-0000-4000-8000-000000000000",
                        "cwd": std::env::temp_dir().to_string_lossy(),
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn loading_in_another_directory_is_refused() {
        // ACP requires the caller's cwd to match the session's, and it has to:
        // the history was produced somewhere.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-cwd-mismatch"),
        );

        let first = agent(scripted(vec![]));
        initialize(&first).await;
        let (session_id, _cwd) = session_with_a_home(&first).await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let error = error_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": std::env::temp_dir().to_string_lossy(),
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn loading_without_naming_a_directory_is_allowed() {
        // A client that named no directory is not disagreeing with the recorded
        // one, so it is not refused here: the app has no directory picker and says
        // so on load the same way it does when it creates a session. Refusing would
        // leave every session from a previous run unloadable, which after a restart
        // is all of them.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-cwd-absent"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, _cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": "",
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx).len(),
            2,
            "the conversation comes back, not an empty session"
        );
    }

    #[tokio::test]
    async fn a_replayed_turn_says_how_it_ended() {
        // A client being shown a session has no `session/prompt` response to close
        // its turns with, so each reason rides on the last update of its turn.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-turn-ends"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "answer".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;
        prompt_titles(&first, &session_id, "again").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        let updates = replayed_turns(&mut rx);
        assert_eq!(updates.len(), 4, "{updates:?}");
        assert_ne!(updates[2].0, updates[0].0, "each turn is its own");
        for turn in [&updates[0..2], &updates[2..4]] {
            assert_eq!(
                turn[0].1, None,
                "only the last update of a turn closes it: {turn:?}"
            );
            assert_eq!(turn[1].1, Some("completed".to_string()), "{turn:?}");
        }
    }

    #[tokio::test]
    async fn wrong_protocol_version_is_invalid_params() {
        let agent = agent(scripted(vec![]));
        let error = error_of(
            agent
                .handle(request(
                    INITIALIZE,
                    json!({ "protocolVersion": 99, "clientCapabilities": {} }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn session_new_returns_an_opaque_id() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let id = new_session(&agent).await;
        assert!(!id.0.is_empty());
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        // Counted rather than asserted as the only one: the configuration
        // directory comes from a process-wide environment variable that tests
        // beside this one also set, so the listing may be reading their home.
        assert!(
            listed["sessions"]
                .as_array()
                .is_some_and(|all| !all.is_empty()),
            "the session it just made is listed"
        );
        // The session works in the workspace created for it, named by its id —
        // not in the directory the client sent.
        let mine = listed["sessions"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|info| info["sessionId"] == json!(id.0.as_ref()))
            .expect("the session it just made is listed");
        let cwd = mine["cwd"]
            .as_str()
            .expect("the session reports a working directory");
        assert!(
            cwd.ends_with(id.0.as_ref()),
            "the workspace is named by the session id: {cwd}"
        );
        assert!(
            std::path::Path::new(cwd).is_dir(),
            "the workspace exists: {cwd}"
        );
    }

    #[tokio::test]
    async fn session_new_works_where_the_client_said_to() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let chosen = std::env::temp_dir().join(format!("srud-chosen-{}", std::process::id()));
        std::fs::create_dir_all(&chosen).expect("a directory to work in");

        let reply = agent
            .handle(request(
                SESSION_NEW,
                json!({ "cwd": chosen.to_str(), "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new should succeed");
        };
        let id = SessionId::new(result["sessionId"].as_str().unwrap().to_string());

        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        assert_eq!(listed["sessions"][0]["cwd"], chosen.to_str().unwrap());

        // A directory the client named is the user's, so deleting the session
        // must leave it standing.
        result_of(
            agent
                .handle(request(
                    SESSION_DELETE,
                    json!({ "sessionId": id.0.as_ref() }),
                ))
                .await,
        );
        assert!(chosen.is_dir(), "{chosen:?} is not the agent's to delete");
    }

    #[tokio::test]
    async fn prompt_streams_updates_and_returns_stop_reason() {
        let agent = agent(scripted(vec![
            ModelEvent::TextDelta { delta: "he".into() },
            ModelEvent::TextDelta {
                delta: "llo".into(),
            },
        ]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let mut rx = agent.subscribe();

        let result = result_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hi" }]
                    }),
                ))
                .await,
        );
        assert_eq!(result["stopReason"], json!("end_turn"));

        let mut kinds = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            assert_eq!(value["params"]["sessionId"], json!(session_id.0.as_ref()));
            let kind = value["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap()
                .to_string();
            // Session-metadata updates are not turn output, so they carry no
            // turn id; everything the turn loop emits does.
            if kind != "session_info_update" {
                assert!(
                    value["params"]["_meta"]["srud"]["turnId"].is_string(),
                    "{kind} carries a turn id"
                );
            }
            kinds.push(kind);
        }
        assert_eq!(
            kinds,
            vec![
                // The first prompt names the session, before any turn output.
                "session_info_update",
                "user_message_chunk",
                "agent_message_chunk",
                "agent_message_chunk"
            ]
        );
    }

    #[tokio::test]
    async fn cancel_makes_prompt_return_cancelled() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let prompt = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        // Give the turn a moment to claim the slot and emit its first delta.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cancel_reply = agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        assert!(matches!(cancel_reply.inner(), Response::Result { .. }));

        let result = result_of(prompt.await.unwrap());
        assert_eq!(result["stopReason"], json!("cancelled"));
    }

    #[tokio::test]
    async fn prompt_again_while_busy_is_session_busy() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let first = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "second" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_BUSY);

        // Clean up: cancel so the in-flight turn finishes.
        agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        let _ = first.await.unwrap();
    }

    #[tokio::test]
    async fn the_first_prompt_names_the_session_and_broadcasts_it() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let titles = prompt_titles(&agent, &session_id, "Why is the timestamp wrong?").await;
        assert_eq!(
            titles,
            vec![Some("Why is the timestamp wrong?".to_string())],
            "the first prompt names the session once"
        );

        // A later turn must not rename it.
        let later = prompt_titles(&agent, &session_id, "A completely different question").await;
        assert!(
            later.is_empty(),
            "a later prompt does not rename the session"
        );
    }

    #[tokio::test]
    async fn set_title_renames_and_broadcasts() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let mut rx = agent.subscribe();
        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": session_id.0.as_ref(), "title": "Chosen by hand" }),
                ))
                .await,
        );
        assert_eq!(result["title"], json!("Chosen by hand"));

        let notification = rx.try_recv().expect("the rename is broadcast");
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["method"], json!("session/update"));
        assert_eq!(
            value["params"]["update"]["sessionUpdate"],
            json!("session_info_update")
        );
        assert_eq!(value["params"]["update"]["title"], json!("Chosen by hand"));

        // A rename outranks the derived name, so the next prompt leaves it alone.
        let titles = prompt_titles(&agent, &session_id, "another question").await;
        assert!(titles.is_empty());
    }

    #[tokio::test]
    async fn a_blank_rename_clears_the_title_and_reports_it() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        agent
            .handle(request(
                SRUD_SESSION_SET_TITLE,
                json!({ "sessionId": session_id.0.as_ref(), "title": "Named" }),
            ))
            .await;

        let mut rx = agent.subscribe();
        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": session_id.0.as_ref(), "title": "  " }),
                ))
                .await,
        );
        assert!(
            result.get("title").is_none(),
            "a blank title is reported as the clear it was"
        );

        let notification = rx.try_recv().expect("the clear is broadcast");
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["params"]["update"]["title"], json!(null));
    }

    #[tokio::test]
    async fn renaming_an_unknown_session_is_session_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": "missing", "title": "x" }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn list_reports_the_derived_title() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        prompt_titles(&agent, &session_id, "Name me").await;

        let result = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        let listed = result["sessions"]
            .as_array()
            .expect("sessions is an array")
            .iter()
            .find(|info| info["sessionId"] == json!(session_id.0.as_ref()))
            .expect("the prompted session is listed");
        assert_eq!(listed["title"], json!("Name me"));
    }

    #[tokio::test]
    async fn unknown_and_unimplemented_methods_are_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        for method in UNIMPLEMENTED_METHODS {
            let error = error_of(agent.handle(request(method, json!({}))).await);
            assert_eq!(i32::from(error.code), METHOD_NOT_FOUND, "for {method}");
        }
        let error = error_of(agent.handle(request("bogus/method", json!({}))).await);
        assert_eq!(i32::from(error.code), METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn prompt_on_unknown_session_is_session_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": "missing",
                        "prompt": [{ "type": "text", "text": "hi" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn close_and_delete_deregister_the_session() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let reply = agent
            .handle(request(
                SESSION_CLOSE,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        assert!(matches!(reply.inner(), Response::Result { .. }));

        let error = error_of(
            agent
                .handle(request(
                    SESSION_DELETE,
                    json!({ "sessionId": session_id.0.as_ref() }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn malformed_params_are_invalid_params() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(SESSION_NEW, json!({ "notCwd": 1 })))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn non_text_prompt_is_invalid_params() {
        let agent = agent(scripted(vec![ModelEvent::TextDelta { delta: "ok".into() }]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Image(srud_protocol::acp::ImageContent::new("data", "image/png")),
        ];
        let params = json!({
            "sessionId": session_id.0.as_ref(),
            "prompt": serde_json::to_value(&blocks).unwrap(),
        });
        let error = error_of(agent.handle(request(SESSION_PROMPT, params)).await);
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[test]
    fn prompt_validation_rejects_images_and_accepts_text() {
        let blocks = vec![ContentBlock::Text(TextContent::new("hello"))];
        assert_eq!(prompt_with_validation(&blocks).unwrap(), "hello");

        let blocks = vec![ContentBlock::Image(srud_protocol::acp::ImageContent::new(
            "d",
            "image/png",
        ))];
        let err = prompt_with_validation(&blocks).unwrap_err();
        assert_eq!(i32::from(err.code), INVALID_PARAMS);
        assert!(err.message.contains("text prompts only"));
    }

    #[test]
    fn reply_helpers_produce_well_formed_envelopes() {
        let ok = reply_ok(RequestId::Number(3), json!({ "a": 1 }));
        let value = serde_json::to_value(&ok).unwrap();
        assert_eq!(value["jsonrpc"], json!("2.0"));
        assert_eq!(value["id"], json!(3));
        assert_eq!(value["result"]["a"], json!(1));

        let err = reply_err(
            RequestId::Str("x".into()),
            AcpError::new(INTERNAL_ERROR, "boom"),
        );
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["error"]["code"], json!(INTERNAL_ERROR));
    }
}
