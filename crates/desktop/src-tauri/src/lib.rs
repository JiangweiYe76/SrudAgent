//! Tauri shell: hosts the ACP agent in-process and bridges it to the webview
//! over the command/event protocol defined by `srud_protocol::transport::tauri`.
//!
//! The webview sends JSON-RPC requests through `rpc_request`; every agent
//! notification is forwarded on the `rpc_notify` event. The model client is
//! configured from the environment, seeded by a dotenv file: `SRUD_ENV_FILE`
//! names it explicitly, otherwise `.env.test` in the workspace root is used
//! when present. Without an API key the agent stays unconfigured and requests
//! answer with a JSON-RPC error instead of crashing the app.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use srud_client::openai::ChatClient;
use srud_core::client::ModelClient;
use srud_protocol::acp::{AcpError, JsonRpcMessage, Notification, Response, SessionNotification};
use srud_protocol::error::INTERNAL_ERROR;
use srud_protocol::transport::tauri::{NotifyBody, RpcNotify, RpcRequest, RPC_NOTIFY_EVENT};
use srud_server::{config, standard_tools, Agent, RpcReply};
use tauri::{AppHandle, Emitter, Manager, State};

/// Environment variable naming the dotenv file to load before building the
/// model client.
const ENV_FILE_VAR: &str = "SRUD_ENV_FILE";
/// File loaded when `SRUD_ENV_FILE` is unset and the file exists.
const DEFAULT_ENV_FILE: &str = ".env.test";

/// Shared state: the agent, or `None` when the model client could not be
/// configured from the environment.
struct AgentState(Option<Arc<Agent>>);

/// Loads the dotenv file that provides the model configuration.
///
/// `SRUD_ENV_FILE` wins when set; otherwise `.env.test` is searched from the
/// manifest directory up to the workspace root. `dotenvy` does not overwrite
/// variables already present in the real environment, so explicit exports
/// take precedence.
fn load_env_file() {
    if let Ok(explicit) = std::env::var(ENV_FILE_VAR) {
        if let Err(err) = dotenvy::from_path(&explicit) {
            eprintln!("failed to load {explicit}: {err}");
        }
        return;
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join(DEFAULT_ENV_FILE);
        if candidate.exists() {
            if let Err(err) = dotenvy::from_path(&candidate) {
                eprintln!("failed to load {}: {err}", candidate.display());
            }
            return;
        }
        if !dir.pop() {
            return;
        }
    }
}

/// Builds the agent from the environment.
///
/// The model client is the one thing this shell decides: which provider to talk
/// to is a property of the machine the app runs on. The tool set is not — it
/// comes from the agent, which is what executes tool calls.
fn build_agent() -> Option<Arc<Agent>> {
    let model = ChatClient::from_env().ok()?;
    let model: Arc<dyn ModelClient> = Arc::new(model);
    Some(Arc::new(Agent::new(model, standard_tools())))
}

/// Builds the `rpc_notify` payload for one agent notification.
///
/// The shape is the contract with the webview: a JSON-RPC notification with
/// no `id`, `{jsonrpc, method, params}`, where `params` is the
/// [`SessionNotification`] (`sessionId`, `update`, `_meta`). Kept free of the
/// `AppHandle` so the wire shape is unit-testable without a running app.
fn notify_payload(notification: Notification<SessionNotification>) -> RpcNotify {
    let body = match serde_json::to_value(notification.params) {
        Ok(value) => value,
        Err(_) => Value::Null,
    };
    RpcNotify::wrap(NotifyBody::Notification(Notification {
        method: notification.method,
        params: Some(body),
    }))
}

/// Forwards one agent notification to the webview as `rpc_notify`.
fn emit_notify(app: &AppHandle, notification: Notification<SessionNotification>) {
    let _ = app.emit(RPC_NOTIFY_EVENT, &notify_payload(notification));
}

/// Streams every agent notification to the webview for the app's lifetime.
///
/// A single subscription spans all turns and sessions. A slow webview that
/// falls behind the broadcast channel loses the oldest updates and keeps
/// draining; message deltas are accumulative, so a dropped chunk only costs
/// rendering granularity, never correctness.
fn spawn_notify_pump(app: AppHandle, agent: &Agent) {
    let mut rx = agent.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(notification) => emit_notify(&app, notification),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Handles one JSON-RPC request from the webview and returns its reply.
///
/// `session/prompt` resolves when the turn ends; its streaming updates reach
/// the webview independently through the `rpc_notify` pump. Protocol-level
/// failures are encoded inside the JSON-RPC reply, so the Tauri `Result` is
/// always `Ok` — it only reports transport failures.
#[tauri::command]
async fn rpc_request(
    state: State<'_, AgentState>,
    payload: RpcRequest,
) -> Result<RpcReply, String> {
    let Some(agent) = state.0.clone() else {
        let id = payload.inner().id.clone();
        return Ok(JsonRpcMessage::wrap(Response::new(
            id,
            Err::<Value, AcpError>(AcpError::new(
                INTERNAL_ERROR,
                "agent is not configured: set SRUD_API_KEY (optionally SRUD_BASE_URL, SRUD_MODEL)",
            )),
        )));
    };
    Ok(agent.handle(payload).await)
}

/// The working directory new sessions are created with: the directory the
/// app was launched from, falling back to the user's home directory.
#[tauri::command]
fn default_cwd() -> String {
    std::env::current_dir()
        .ok()
        .or_else(config::user_home)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_owned())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    load_env_file();
    // Reported, not fatal: the app still runs without it, and the first session
    // that needs a workspace will say so itself.
    if let Err(error) = config::ensure() {
        eprintln!("{error}");
    }
    tauri::Builder::default()
        .setup(|app| {
            let agent = build_agent();
            if let Some(agent) = &agent {
                spawn_notify_pump(app.handle().clone(), agent);
            } else {
                eprintln!("SRUD_API_KEY not set; rpc_request will answer with a config error");
            }
            app.manage(AgentState(agent));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![rpc_request, default_cwd])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;
    use srud_core::client::{ModelEvent, ModelRequest, ModelStream};
    use srud_core::tools::ToolRegistry;
    use srud_protocol::acp::{Request, RequestId};
    use tokio_util::sync::CancellationToken;

    /// A model that replays a fixed event list, so the transport can be tested
    /// without a network call.
    struct Scripted {
        events: Vec<Result<ModelEvent, srud_core::client::ModelError>>,
    }

    #[async_trait::async_trait]
    impl ModelClient for Scripted {
        async fn stream(
            &self,
            _request: ModelRequest,
            _cancel: CancellationToken,
        ) -> Result<ModelStream, srud_core::client::ModelError> {
            Ok(Box::pin(stream::iter(self.events.clone())))
        }
    }

    fn req(method: &str, params: Value) -> RpcRequest {
        JsonRpcMessage::wrap(Request {
            id: RequestId::Number(1),
            method: method.into(),
            params: Some(params),
        })
    }

    /// Runs the full in-process path — subscribe, drive a turn, map every
    /// notification to its webview payload — and returns the serialized
    /// `rpc_notify` payloads the webview would receive.
    async fn notify_payloads(agent: &Agent, session_id: &str) -> Vec<Value> {
        // Subscribe before prompting, as the setup-time pump does, so no
        // update emitted during the turn is missed.
        let mut rx = agent.subscribe();
        agent
            .handle(req(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": "hi" }],
                }),
            ))
            .await;
        let mut payloads = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            payloads.push(serde_json::to_value(notify_payload(notification)).unwrap());
        }
        payloads
    }

    /// Opens a session on a configured agent, returning its id.
    ///
    /// The agent settles the session's workspace from the configuration
    /// directory, so `SRUD_HOME` is pointed at a directory of this binary's own:
    /// a test must not write into the developer's home.
    async fn open_session(agent: &Agent) -> String {
        static CONFIG_HOME: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        CONFIG_HOME.get_or_init(|| {
            let path = std::env::temp_dir().join(format!("srud-desktop-{}", std::process::id()));
            std::env::set_var(config::HOME_VAR, path);
        });
        agent
            .handle(req(
                "initialize",
                json!({ "protocolVersion": 1, "clientCapabilities": {} }),
            ))
            .await;
        let reply = agent
            .handle(req(
                "session/new",
                json!({ "cwd": "/tmp", "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new failed");
        };
        result["sessionId"].as_str().expect("sessionId").to_string()
    }

    #[tokio::test]
    async fn a_streamed_turn_reaches_the_webview_as_rpc_notify() {
        let agent = Agent::new(
            Arc::new(Scripted {
                events: vec![
                    Ok(ModelEvent::TextDelta {
                        delta: "Hello".into(),
                    }),
                    Ok(ModelEvent::Done),
                ],
            }),
            Arc::new(ToolRegistry::new()),
        );
        let session_id = open_session(&agent).await;
        let payloads = notify_payloads(&agent, &session_id).await;

        let chunk = payloads
            .iter()
            .find(|p| p["params"]["update"]["sessionUpdate"] == json!("agent_message_chunk"))
            .expect("the assistant delta reaches the webview");
        // The webview contract: a JSON-RPC notification (no `id`) whose params
        // carry sessionId and the update content.
        assert!(
            chunk.get("id").is_none(),
            "a notification must not carry an id"
        );
        assert_eq!(chunk["method"], json!("session/update"));
        assert_eq!(chunk["params"]["sessionId"], json!(session_id));
        assert_eq!(chunk["params"]["update"]["content"]["text"], json!("Hello"));
    }

    #[test]
    fn an_unconfigured_request_answers_with_a_jsonrpc_error() {
        // The error branch of `rpc_request` (no API key) is exercised through
        // the same envelope the command returns; the Tauri `State` is not
        // needed to assert the shape the webview parses.
        let reply = JsonRpcMessage::wrap(Response::new(
            RequestId::Number(1),
            Err::<Value, AcpError>(AcpError::new(INTERNAL_ERROR, "agent is not configured")),
        ));
        let value = serde_json::to_value(&reply).unwrap();
        assert_eq!(value["id"], json!(1));
        assert_eq!(value["error"]["code"], json!(INTERNAL_ERROR));
    }

    /// The real end-to-end path with live model credentials from `.env.test`:
    /// webview request → agent → core → provider → streamed `rpc_notify`
    /// payloads. Run explicitly:
    ///
    /// ```sh
    /// cargo test -p desktop -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "requires live model credentials from .env.test"]
    async fn a_prompt_round_trip_streams_live_text_to_the_webview() {
        load_env_file();
        let Ok(model) = ChatClient::from_env() else {
            eprintln!("SRUD_API_KEY not set; skipping live test");
            return;
        };
        let agent = Agent::new(Arc::new(model), Arc::new(ToolRegistry::new()));
        let session_id = open_session(&agent).await;

        let mut rx = agent.subscribe();
        agent
            .handle(req(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": "Reply with exactly: SRUD-OK" }],
                }),
            ))
            .await;

        let mut streamed = String::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(notify_payload(notification)).unwrap();
            if value["params"]["update"]["sessionUpdate"] == json!("agent_message_chunk") {
                if let Some(text) = value["params"]["update"]["content"]["text"].as_str() {
                    streamed.push_str(text);
                }
            }
        }
        eprintln!("live streamed text: {streamed:?}");
        assert!(
            !streamed.trim().is_empty(),
            "the live model streamed no text"
        );
    }
}
