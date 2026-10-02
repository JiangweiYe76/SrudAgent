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
//! | `session/prompt`  | run one turn; the reply is the turn's end            |
//! | `session/cancel`  | interrupt the active turn                            |
//! | `session/list`    | snapshot live sessions                               |
//! | `session/close`   | interrupt + deregister                               |
//! | `session/delete`  | deregister                                           |
//! | `_srud/unstable/session/set_title` | rename a session               |
//!
//! Everything else — `session/load`, `session/resume`, config/mode setters,
//! `authenticate`, and the remaining `_srud/unstable/*` extensions — answers
//! `METHOD_NOT_FOUND`, matching what the advertised capabilities promise.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::Value;
use srud_core::client::ModelClient;
use srud_core::session::Session;
use srud_core::tools::ToolRegistry;
use srud_protocol::acp::methods::{
    INITIALIZE, SESSION_CANCEL, SESSION_CLOSE, SESSION_DELETE, SESSION_LIST, SESSION_NEW,
    SESSION_PROMPT, SRUD_SESSION_SET_TITLE,
};
use srud_protocol::acp::{
    AcpError, CancelNotification, CloseSessionRequest, CloseSessionResponse, ContentBlock,
    DeleteSessionRequest, DeleteSessionResponse, Implementation, InitializeRequest,
    InitializeResponse, JsonRpcMessage, ListSessionsResponse, MaybeUndefined, NewSessionRequest,
    NewSessionResponse, Notification, PromptRequest, PromptResponse, ProtocolVersion, Request,
    RequestId, Response, SessionId, SessionInfoUpdate, SessionNotification, SessionUpdate,
    CLIENT_METHOD_NAMES,
};
use srud_protocol::error::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, SESSION_BUSY,
    SESSION_NOT_FOUND,
};
use srud_protocol::srud::methods::{SetSessionTitleRequest, SetSessionTitleResponse};
use srud_protocol::srud::turn_end::TurnEndWire;

use crate::convert::{prompt_outcome, prompt_text};
use crate::events::EventHub;
use crate::sessions::SessionManager;

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
            SESSION_NEW => self.handle_new_session(params).map(serialize),
            SESSION_PROMPT => self.handle_prompt(params).await.map(serialize),
            SESSION_CANCEL => self
                .handle_cancel(params)
                .map(|()| Value::Object(Default::default())),
            SESSION_LIST => self.handle_list().map(serialize),
            SESSION_CLOSE => self
                .handle_close(params)
                .map(|()| serialize(CloseSessionResponse::new())),
            SESSION_DELETE => self
                .handle_delete(params)
                .map(|()| serialize(DeleteSessionResponse::new())),
            SRUD_SESSION_SET_TITLE => self.handle_set_title(params).map(serialize),
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

    fn handle_new_session(&self, params: Value) -> Result<NewSessionResponse, AcpError> {
        // Parsed for validation only: the session's working directory is the
        // workspace this agent creates for it, not the directory the client
        // happens to have open.
        let _request: NewSessionRequest = parse_params(params)?;
        let session_id = self.sessions.create().map_err(|err| {
            AcpError::new(
                INTERNAL_ERROR,
                format!("could not create the session workspace: {err}"),
            )
        })?;
        Ok(NewSessionResponse::new(session_id))
    }

    async fn handle_prompt(&self, params: Value) -> Result<PromptResponse, AcpError> {
        let request: PromptRequest = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        let text = prompt_with_validation(&request.prompt)?;
        self.name_from_first_prompt(&request.session_id, &text);

        let sink = self.hub.sink_for(session.id());
        let result = srud_core::turn::run_turn(
            &session,
            srud_core::types::TurnInput { text },
            self.model.as_ref(),
            self.tools.as_ref(),
            &sink,
        )
        .await
        .map_err(|_| AcpError::new(SESSION_BUSY, "session already has an active turn"))?;

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

    fn handle_list(&self) -> Result<ListSessionsResponse, AcpError> {
        Ok(ListSessionsResponse::new(self.sessions.list()))
    }

    fn handle_close(&self, params: Value) -> Result<(), AcpError> {
        let request: CloseSessionRequest = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        session.interrupt();
        self.sessions.remove(&request.session_id);
        Ok(())
    }

    fn handle_delete(&self, params: Value) -> Result<(), AcpError> {
        let request: DeleteSessionRequest = parse_params(params)?;
        if !self.sessions.remove(&request.session_id) {
            return Err(session_not_found(&request.session_id));
        }
        Ok(())
    }

    /// Names an unnamed session after the message that opened it.
    ///
    /// A session is named by its first prompt and never re-derived: later turns
    /// must not overwrite it, and a rename the user has already made wins. When
    /// this call is the one that named the session, it broadcasts the title so
    /// every attached client renders it without asking.
    fn name_from_first_prompt(&self, session_id: &SessionId, text: &str) {
        let title = title_from_prompt(text);
        if self.sessions.name_if_unnamed(session_id, &title) {
            self.broadcast_title(session_id, Some(title));
        }
    }

    fn handle_set_title(&self, params: Value) -> Result<SetSessionTitleResponse, AcpError> {
        let request: SetSessionTitleRequest = parse_params(params)?;
        if !self.sessions.set_title(&request.session_id, &request.title) {
            return Err(session_not_found(&request.session_id));
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
        AGENT_METHOD_NAMES.session_load,
        AGENT_METHOD_NAMES.session_resume,
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
        let reply = agent
            .handle(request(
                SESSION_NEW,
                json!({ "cwd": "/tmp/srud-test", "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new should succeed");
        };
        SessionId::new(result["sessionId"].as_str().unwrap().to_string())
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
        assert_eq!(result["agentCapabilities"]["loadSession"], json!(false));
        assert!(result["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
        assert!(result.get("authMethods").is_some());
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
        assert_eq!(listed["sessions"].as_array().unwrap().len(), 1);
        // The session works in the workspace created for it, named by its id —
        // not in the directory the client sent.
        let cwd = listed["sessions"][0]["cwd"]
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
