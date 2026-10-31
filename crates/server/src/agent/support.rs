//! Fixtures the agent's unit tests share.

use super::*;
use crate::config;
use crate::providers::{Model, Provider, ProviderInput};
use crate::test_env::{self, Guard};
use futures::{stream, StreamExt};
use serde_json::json;
use srud_core::client::{ModelEvent, ModelRequest, ModelStream};
use srud_protocol::acp::methods::{SRUD_SESSION_FORK, SRUD_SESSION_ROLLOUT_READ};
use srud_protocol::acp::AGENT_METHOD_NAMES;
use srud_protocol::transport::tauri::RpcRequest;
use tokio_util::sync::CancellationToken;

/// The method names that answer `METHOD_NOT_FOUND`, so tests can assert
/// the coverage boundary.
pub(super) const UNIMPLEMENTED_METHODS: &[&str] = &[
    AGENT_METHOD_NAMES.session_set_mode,
    AGENT_METHOD_NAMES.authenticate,
    SRUD_SESSION_FORK,
    SRUD_SESSION_ROLLOUT_READ,
];

/// Prompts a session, drains its turn, and returns the broadcast titles.
///
/// The subscriber is attached before the prompt so the title broadcast on
/// the first turn cannot be missed.
pub(super) async fn prompt_titles(
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
        if value["params"]["update"]["sessionUpdate"] == serde_json::json!("session_info_update") {
            titles.push(
                value["params"]["update"]["title"]
                    .as_str()
                    .map(ToString::to_string),
            );
        }
    }
    titles
}

pub(super) struct Scripted {
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

pub(super) fn scripted(events: Vec<ModelEvent>) -> Arc<dyn ModelClient> {
    Arc::new(Scripted {
        events,
        stall_until_cancel: false,
    })
}

pub(super) fn stalling(events: Vec<ModelEvent>) -> Arc<dyn ModelClient> {
    Arc::new(Scripted {
        events,
        stall_until_cancel: true,
    })
}

pub(super) fn agent(model: Arc<dyn ModelClient>) -> Agent {
    Agent::new(model, Arc::new(ToolRegistry::new()))
}

pub(super) fn request(method: &str, params: Value) -> RpcRequest {
    JsonRpcMessage::wrap(Request {
        id: RequestId::Number(1),
        method: method.into(),
        params: Some(params),
    })
}

pub(super) async fn initialize(agent: &Agent) {
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

pub(super) async fn new_session(agent: &Agent) -> SessionId {
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
pub(super) async fn new_session_in(agent: &Agent, cwd: &str) -> SessionId {
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
pub(super) fn conversation(
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
/// session rather than to a turn. A replayed update carries no turn end — that
/// is on the `session/load` response — so the second half is always `None`.
pub(super) fn replayed_updates(
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

pub(super) fn result_of(reply: RpcReply) -> Value {
    match reply.into_inner() {
        Response::Result { result, .. } => result,
        Response::Error { error, .. } => panic!("expected success, got {error:?}"),
    }
}

pub(super) fn error_of(reply: RpcReply) -> AcpError {
    match reply.into_inner() {
        Response::Error { error, .. } => error,
        Response::Result { result, .. } => panic!("expected error, got {result:?}"),
    }
}

/// A session and the directory it works in, on a configuration directory of
/// this test's own.
///
/// Returned together because a load has to name the directory ACP will check
/// against, and the caller has to keep the guard alive for it to be found.
pub(super) async fn session_with_a_home(agent: &Agent) -> (SessionId, String) {
    let cwd = std::env::temp_dir().join(format!("srud-load-cwd-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).expect("a directory to work in");
    let cwd = cwd.to_string_lossy().into_owned();
    let id = new_session_in(agent, &cwd).await;
    (id, cwd)
}

/// A factory that records the references it was asked to build and answers
/// every one of them with the same scripted client.
pub(super) struct RecordingFactory {
    events: Vec<ModelEvent>,
    built: std::sync::Mutex<Vec<String>>,
}

impl RecordingFactory {
    pub(super) fn new(events: Vec<ModelEvent>) -> Arc<Self> {
        Arc::new(Self {
            events,
            built: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// The references built so far, in order.
    pub(super) fn built(&self) -> Vec<String> {
        self.built.lock().expect("factory lock poisoned").clone()
    }
}

impl ModelFactory for RecordingFactory {
    fn build(
        &self,
        provider: &Provider,
        model: &Model,
        _thinking: Option<srud_core::thinking::ThinkingRequest>,
    ) -> Result<Arc<dyn ModelClient>, srud_core::client::ModelError> {
        self.built
            .lock()
            .expect("factory lock poisoned")
            .push(format!("{}/{}", provider.id, model.id));
        Ok(Arc::new(Scripted {
            events: self.events.clone(),
            stall_until_cancel: false,
        }))
    }
}

pub(super) fn sample_provider(id: &str, models: &[&str]) -> ProviderInput {
    ProviderInput {
        id: id.to_owned(),
        name: format!("Provider {id}"),
        base_url: format!("https://{id}.example/v1"),
        api_key: Some(format!("sk-{id}-secret-key")),
        models: models
            .iter()
            .map(|model| Model {
                id: (*model).to_owned(),
                name: None,
                modalities: Vec::new(),
                context_window: None,
                thinking: None,
            })
            .collect(),
    }
}

/// An agent over a two-provider store, and the factory it resolves with.
///
/// The configuration directory is handed back before this returns: the store
/// has already read the file, and holding the environment across the caller's
/// own session setup would deadlock two guards against each other.
pub(super) fn providers_agent(events: Vec<ModelEvent>) -> (Agent, Arc<RecordingFactory>) {
    providers_agent_with(events, true)
}

/// As [`providers_agent`], but `fixed` decides whether the host also left a
/// client behind for sessions that name no model. A host with nothing but a
/// provider file leaves none.
pub(super) fn providers_agent_with(
    events: Vec<ModelEvent>,
    fixed: bool,
) -> (Agent, Arc<RecordingFactory>) {
    let (store, factory) = {
        let _env = Guard::take(&[config::HOME_VAR]);
        std::env::set_var(
            config::HOME_VAR,
            test_env::unique_dir("srud-agent-providers"),
        );
        let store = ProviderStore::load().expect("an empty store");
        store
            .upsert(sample_provider("openai", &["small", "large"]))
            .expect("a valid provider");
        store
            .upsert(sample_provider("anthropic", &["opus"]))
            .expect("a valid provider");
        (store, RecordingFactory::new(events))
    };
    let model = if fixed {
        Some(scripted(Vec::new()) as Arc<dyn ModelClient>)
    } else {
        None
    };
    let agent = Agent::with_providers(
        model,
        Arc::new(ToolRegistry::new()),
        ModelProviders::new(Arc::new(store), factory.clone()),
    );
    (agent, factory)
}

/// Points the configuration directory at one of this test's own, so a session
/// is created under the system temp directory rather than the developer's
/// home. Kept alive for the length of the test.
pub(super) fn home() -> Guard {
    let guard = Guard::take(&[config::HOME_VAR]);
    std::env::set_var(config::HOME_VAR, test_env::unique_dir("srud-agent-home"));
    guard
}

pub(super) async fn prompt_once(agent: &Agent, session_id: &SessionId) {
    result_of(
        agent
            .handle(request(
                SESSION_PROMPT,
                json!({
                    "sessionId": session_id.0.as_ref(),
                    "prompt": [{ "type": "text", "text": "hello" }],
                }),
            ))
            .await,
    );
}

pub(super) async fn choose(agent: &Agent, session_id: &SessionId, reference: &str) -> Value {
    result_of(
        agent
            .handle(request(
                SESSION_SET_CONFIG_OPTION,
                json!({
                    "sessionId": session_id.0.as_ref(),
                    "configId": "model",
                    "value": reference,
                }),
            ))
            .await,
    )
}

pub(super) async fn new_session_result(agent: &Agent) -> Value {
    result_of(
        agent
            .handle(request(SESSION_NEW, json!({ "cwd": "", "mcpServers": [] })))
            .await,
    )
}

/// The id of the turn a prompt ran, read from the updates it sent.
pub(super) async fn turn_id_of(agent: &Agent, session_id: &SessionId, text: &str) -> String {
    let mut rx = agent.subscribe();
    let _ = agent
        .handle(request(
            SESSION_PROMPT,
            json!({
                "sessionId": session_id.0.as_ref(),
                "prompt": [{ "type": "text", "text": text }]
            }),
        ))
        .await;
    let mut id = String::new();
    while let Ok(notification) = rx.try_recv() {
        let value = serde_json::to_value(&notification).unwrap();
        if let Some(turn_id) = value["params"]["_meta"]["srud"]["turnId"].as_str() {
            id = turn_id.to_string();
        }
    }
    id
}
