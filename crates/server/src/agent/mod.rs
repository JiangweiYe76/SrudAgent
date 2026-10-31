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
//! | `session/set_config_option` | choose the model a session runs on          |
//! | `_srud/unstable/session/set_title` | rename a session               |
//! | `_srud/unstable/provider/list` | list providers, with keys masked      |
//! | `_srud/unstable/provider/set` | add or replace a provider             |
//! | `_srud/unstable/provider/delete` | remove a provider                   |
//! | `_srud/unstable/provider/set_default` | choose new sessions' model     |
//!
//! Everything else — `session/set_mode`, `authenticate`, and the remaining
//! `_srud/unstable/*` extensions — answers `METHOD_NOT_FOUND`, matching what the
//! advertised capabilities promise.
//!
//! # Where a model client comes from
//!
//! A session names the model it runs on as a `providerId/modelId` reference, and
//! the agent resolves it per turn rather than holding one client for its whole
//! life. The reference travels to a client over ACP's own `configOptions`, so a
//! third-party client renders the same selector this one does; the credentials
//! behind it never leave this machine.
//!
//! An agent without a [`ProviderStore`] is one whose client was fixed at
//! construction: [`Agent::new`] takes one, which is what a test and an
//! environment-configured host both want. [`Agent::with_providers`] adds the
//! resolvable path on top.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use srud_core::client::ModelClient;
use srud_core::session::Session;
use srud_core::tools::ToolRegistry;
use srud_protocol::acp::methods::{
    INITIALIZE, SESSION_CANCEL, SESSION_CLOSE, SESSION_DELETE, SESSION_LIST, SESSION_LOAD,
    SESSION_NEW, SESSION_PROMPT, SESSION_RESUME, SESSION_SET_CONFIG_OPTION, SRUD_PROVIDER_DELETE,
    SRUD_PROVIDER_LIST, SRUD_PROVIDER_SET, SRUD_PROVIDER_SET_DEFAULT, SRUD_SESSION_REGENERATE,
    SRUD_SESSION_SET_TITLE, SRUD_SESSION_STEER,
};
use srud_protocol::acp::{
    AcpError, CloseSessionResponse, ContentBlock, DeleteSessionResponse, Implementation,
    InitializeRequest, InitializeResponse, JsonRpcMessage, MaybeUndefined, Notification,
    ProtocolVersion, Request, RequestId, Response, SessionId, SessionInfoUpdate,
    SessionNotification, SessionUpdate, CLIENT_METHOD_NAMES,
};
use srud_protocol::error::{INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, SESSION_NOT_FOUND};

use crate::events::{EventHub, TurnSink};
use crate::providers::{ModelFactory, ProviderStore};
use crate::sessions::SessionManager;
mod model;
mod provider;
mod restore;
mod session;
#[cfg(test)]
mod support;
mod turn;

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

/// The provider configuration an agent resolves a session's model against.
///
/// The two halves belong together: the store says which providers exist and what
/// they offer, and the factory builds a client for one of them. Neither is much
/// use alone, and an agent has both or neither.
pub struct ModelProviders {
    /// The configured providers, and the credentials behind them.
    pub store: Arc<ProviderStore>,
    /// Builds a model client for a provider the store names.
    pub factory: Arc<dyn ModelFactory>,
}

impl ModelProviders {
    /// Pairs a provider store with the factory that builds its clients.
    #[must_use]
    pub fn new(store: Arc<ProviderStore>, factory: Arc<dyn ModelFactory>) -> Self {
        Self { store, factory }
    }
}

/// An ACP v1 agent backed by the core runtime.
pub struct Agent {
    /// The client used when a session names no model: one older than the
    /// provider file, which ran on whatever the host had fixed. `None` when the
    /// host fixed nothing, which is only possible alongside providers — there,
    /// every session that can run names its own model. Also the whole answer for
    /// an agent built without providers.
    model: Option<Arc<dyn ModelClient>>,
    /// The configurable providers, when the host supplied them.
    providers: Option<ModelProviders>,
    tools: Arc<ToolRegistry>,
    sessions: Arc<SessionManager>,
    hub: EventHub,
    active_sinks: Mutex<HashMap<SessionId, TurnSink>>,
    initialized: AtomicBool,
    /// Set once the host has begun shutting down. New prompts and steers are
    /// refused from that point on, so a drain cannot be extended by new work.
    shutting_down: AtomicBool,
    /// Held for reading by every prompt; [`Agent::shutdown`] probes the write
    /// side to see whether any is left.
    ///
    /// The write side can be taken only once every in-flight prompt has left,
    /// so a successful probe means every turn has settled.
    prompt_gate: tokio::sync::RwLock<()>,
}

impl Agent {
    /// Creates an agent over a model client and a tool registry.
    ///
    /// Every session runs on the given client: there is nothing to choose
    /// between, so `session/new` advertises no model selector and
    /// `session/set_config_option` answers `METHOD_NOT_FOUND`.
    #[must_use]
    pub fn new(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            model: Some(model),
            providers: None,
            tools,
            sessions: Arc::new(SessionManager::new()),
            hub: EventHub::new(),
            active_sinks: Mutex::new(HashMap::new()),
            initialized: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            prompt_gate: tokio::sync::RwLock::new(()),
        }
    }

    /// Creates an agent that resolves each session's model from a provider file.
    ///
    /// `model` is the fallback for a session that names no model — one created
    /// before any provider was configured, or restored from a log written then.
    /// `None` says the host has nothing to fall back to, which a host with
    /// providers genuinely can: a session that has no model to run on is told to
    /// choose one, rather than run on a model nobody picked.
    #[must_use]
    pub fn with_providers(
        model: Option<Arc<dyn ModelClient>>,
        tools: Arc<ToolRegistry>,
        providers: ModelProviders,
    ) -> Self {
        Self {
            model,
            providers: Some(providers),
            tools,
            sessions: Arc::new(SessionManager::new()),
            hub: EventHub::new(),
            active_sinks: Mutex::new(HashMap::new()),
            initialized: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            prompt_gate: tokio::sync::RwLock::new(()),
        }
    }

    /// Subscribes to the outgoing `session/update` notifications.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Notification<SessionNotification>> {
        self.hub.subscribe()
    }

    /// Stops accepting new prompts and steers, and waits for every in-flight
    /// turn to settle.
    ///
    /// Active turns are interrupted and re-interrupted while the drain waits,
    /// because a prompt that had already passed its entry check may claim its
    /// turn after the first signal.
    ///
    /// Returns whether every in-flight prompt finished before `timeout`.
    /// A turn that did not settle is left open in the log, exactly as after a
    /// process crash; the next load or resume closes it with a synthetic
    /// interruption.
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        self.shutting_down.store(true, Ordering::Release);

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            self.sessions.interrupt_all();

            // Every prompt holds a read guard for its whole turn, so a write
            // that becomes available means none is left. Probing rather than
            // awaiting the lock keeps the loop free to re-signal a turn that
            // claims the session late.
            if self.prompt_gate.try_write().is_ok() {
                return true;
            }

            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
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
            SESSION_SET_CONFIG_OPTION => self.handle_set_config_option(params).await.map(serialize),
            SRUD_SESSION_SET_TITLE => self.handle_set_title(params).await.map(serialize),
            SRUD_SESSION_STEER => self.handle_steer(params).await.map(serialize),
            SRUD_SESSION_REGENERATE => self.handle_regenerate(params).await.map(serialize),
            SRUD_PROVIDER_LIST => self.handle_provider_list(params),
            SRUD_PROVIDER_SET => self.handle_provider_set(params),
            SRUD_PROVIDER_DELETE => self.handle_provider_delete(params),
            SRUD_PROVIDER_SET_DEFAULT => self.handle_provider_set_default(params),
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
    /// An audio block, refused while `promptCapabilities.audio` is false.
    Audio,
    /// Any other block the agent does not read, such as a resource or a link.
    Other(&'static str),
}

/// The parts of a prompt the agent accepts.
#[derive(Debug, Default)]
pub struct PromptParts {
    /// The text blocks, joined in the order they arrived.
    pub text: String,
    /// The image blocks, in the order they arrived.
    pub images: Vec<srud_core::types::ImagePart>,
}

/// Splits a prompt into the text and images the agent accepts.
///
/// Returns an error naming the first block the agent cannot accept — dropping
/// multimodal input silently would make the model answer a prompt the user
/// never sent.
pub fn prompt_parts(blocks: &[ContentBlock]) -> Result<PromptParts, AcpError> {
    let mut parts = PromptParts::default();
    for block in blocks {
        match block {
            ContentBlock::Text(content) => {
                if !parts.text.is_empty() {
                    parts.text.push('\n');
                }
                parts.text.push_str(&content.text);
            }
            ContentBlock::Image(content) => parts.images.push(srud_core::types::ImagePart {
                mime: content.mime_type.clone(),
                data: content.data.clone(),
            }),
            block => {
                let kind = match block {
                    ContentBlock::Audio(_) => UnsupportedContent::Audio,
                    ContentBlock::Resource(_) => UnsupportedContent::Other("resource"),
                    ContentBlock::ResourceLink(_) => UnsupportedContent::Other("resource_link"),
                    _ => UnsupportedContent::Other("unknown"),
                };
                return Err(AcpError::new(
                    INVALID_PARAMS,
                    format!("this agent cannot accept {kind:?} prompts"),
                ));
            }
        }
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::support::*;
    use crate::agent::*;

    use serde_json::json;
    use srud_protocol::acp::methods::{INITIALIZE, SESSION_NEW};
    use srud_protocol::acp::{AcpError, RequestId, TextContent};
    use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND};

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

    #[test]
    fn prompt_parts_accepts_text_and_images_and_rejects_audio() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("hello")),
            ContentBlock::Image(srud_protocol::acp::ImageContent::new("d", "image/png")),
        ];
        let parts = prompt_parts(&blocks).unwrap();
        assert_eq!(parts.text, "hello");
        assert_eq!(
            parts.images,
            vec![srud_core::types::ImagePart {
                mime: "image/png".into(),
                data: "d".into(),
            }]
        );

        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Audio(srud_protocol::acp::AudioContent::new("d", "audio/wav")),
        ];
        let err = prompt_parts(&blocks).unwrap_err();
        assert_eq!(i32::from(err.code), INVALID_PARAMS);
        assert!(err.message.contains("Audio"));
    }
}
