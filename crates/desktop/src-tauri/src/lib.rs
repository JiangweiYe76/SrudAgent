//! Tauri shell: hosts the ACP agent in-process and bridges it to the webview
//! over the command/event protocol defined by `srud_protocol::transport::tauri`.
//!
//! The webview sends JSON-RPC requests through `rpc_request`; every agent
//! notification is forwarded on the `rpc_notify` event.
//!
//! # Where the model comes from
//!
//! One source: the provider file — `providers.json`, under the configuration
//! directory. It is the one the settings panel writes, and an agent built over
//! it lets each session choose the model it runs on.
//!
//! A provider file that cannot be read is reported, and the app starts from an
//! empty store: the file itself is left untouched until the next edit writes
//! it. The one store is shared between the settings panel and the agent, so a
//! provider added at runtime takes effect without a restart.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::Value;
use srud_client::openai::{ChatClient, ClientConfig};
use srud_core::client::{ModelClient, ModelError};
use srud_protocol::acp::{AcpError, JsonRpcMessage, Notification, Response, SessionNotification};
use srud_protocol::error::INTERNAL_ERROR;
use srud_protocol::transport::tauri::{NotifyBody, RpcNotify, RpcRequest, RPC_NOTIFY_EVENT};
use srud_server::{
    config, standard_tools, Agent, Model, ModelFactory, ModelProviders, Provider, ProviderStore,
    RpcReply,
};
use tauri::{AppHandle, Emitter, Manager, State};

/// Shared state: the agent the webview's requests go to.
struct AgentState(Arc<Agent>);

/// How far the exit hook has progressed.
///
/// A second close request while draining is held off, so it cannot cut the
/// wait short; only the drain task moves the state to `READY`.
struct ShutdownState(AtomicU8);

const SHUTDOWN_RUNNING: u8 = 0;
const SHUTDOWN_DRAINING: u8 = 1;
const SHUTDOWN_READY: u8 = 2;

/// How long a close waits for in-flight turns to settle.
///
/// Long enough for an interrupted turn to flush its partial message, short
/// enough that a provider stuck on a network read cannot keep the window from
/// closing.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

impl Default for ShutdownState {
    fn default() -> Self {
        Self(AtomicU8::new(SHUTDOWN_RUNNING))
    }
}

/// Shared state: the provider store the panel writes through. The agent
/// resolves its sessions against the same store, so an edit there is visible
/// here without a rebuild.
struct ProviderState(Arc<ProviderStore>);

/// Builds a client that talks to a configured provider.
///
/// This is the one place the two halves of a provider — the endpoint and the
/// credential — are put together. The store never does it, so the credentials
/// stay inside the shell; and the server never does it, so it needs no dependency
/// on the OpenAI adapter.
struct ChatFactory;

impl ModelFactory for ChatFactory {
    fn build(
        &self,
        provider: &Provider,
        model: &Model,
        thinking: Option<srud_core::thinking::ThinkingRequest>,
    ) -> Result<Arc<dyn ModelClient>, ModelError> {
        let settings = ClientConfig::new(provider.api_key.clone(), model.id.clone())
            .with_base_url(provider.base_url.clone())
            .with_thinking(thinking);
        Ok(Arc::new(ChatClient::new(settings)))
    }
}

/// The client a session runs on when its log names no model.
///
/// The model new sessions start from, so a session that predates the provider
/// file runs on what its own selector already shows rather than on something
/// the panel does not name. `None` when no default has been chosen, which is a
/// state the app is expected to be in: the session is told to pick a model
/// instead of being handed one.
fn starting_client(
    store: &ProviderStore,
    factory: &dyn ModelFactory,
) -> Option<Arc<dyn ModelClient>> {
    let reference = store.start_reference()?;
    let (provider, model) = store.resolve(&reference).ok()?;
    factory
        .build(&provider, &model, model.thinking_request(None))
        .ok()
}

/// Builds the agent over the shared provider store.
///
/// The store is the one the settings panel writes through, so providers added
/// at runtime are visible to the agent without a rebuild. An empty store is a
/// state the app is expected to start in: the agent still answers, and a
/// session with no model is told to pick one rather than being handed one.
fn build_agent(store: Arc<ProviderStore>) -> Arc<Agent> {
    let tools = standard_tools();
    let factory: Arc<dyn ModelFactory> = Arc::new(ChatFactory);

    // The fallback client is what a session that names no model runs on: the
    // default the store points at, when it points at anything. Having none is
    // not fatal — the store can be filled later through the panel.
    let model: Option<Arc<dyn ModelClient>> = starting_client(&store, factory.as_ref());
    Arc::new(Agent::with_providers(
        model,
        tools,
        ModelProviders::new(store, factory),
    ))
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

/// Handles a provider management command directly through the store.
///
/// Provider commands are answered from the store rather than routed through
/// the agent's dispatch: it is the same store the agent resolves sessions
/// against, so an edit here reaches the next turn through one shared lock.
async fn handle_provider_command(
    store: &ProviderStore,
    payload: &RpcRequest,
) -> Result<RpcReply, String> {
    use srud_protocol::srud::methods::{
        DeleteProviderRequest, ListProvidersRequest, SetDefaultProviderRequest, SetProviderRequest,
    };
    use srud_server::providers::{Model as ServerModel, ProviderInput as ServerProviderInput};

    let id = payload.inner().id.clone();
    let method: &str = &payload.inner().method;
    let params = payload.inner().params.as_ref();

    let catalog = match method {
        "_srud/unstable/provider/list" => {
            let _request: ListProvidersRequest = parse_params(params).map_err(|e| e.to_string())?;
            store.snapshot()
        }
        "_srud/unstable/provider/set" => {
            let request: SetProviderRequest = parse_params(params).map_err(|e| e.to_string())?;
            let input = ServerProviderInput {
                id: request.provider.id,
                name: request.provider.name,
                base_url: request.provider.base_url,
                api_key: request.provider.api_key,
                models: request
                    .provider
                    .models
                    .into_iter()
                    .map(|m| ServerModel {
                        id: m.id,
                        name: m.name,
                        modalities: m.modalities,
                        context_window: m.context_window,
                        thinking: m.thinking,
                    })
                    .collect(),
            };
            store.upsert(input).map_err(|e| e.to_string())?
        }
        "_srud/unstable/provider/delete" => {
            let request: DeleteProviderRequest = parse_params(params).map_err(|e| e.to_string())?;
            store
                .delete(&request.provider_id)
                .map_err(|e| e.to_string())?
        }
        "_srud/unstable/provider/set_default" => {
            let request: SetDefaultProviderRequest =
                parse_params(params).map_err(|e| e.to_string())?;
            store
                .set_default(&request.model)
                .map_err(|e| e.to_string())?
        }
        _ => {
            return Ok(JsonRpcMessage::wrap(Response::new(
                id,
                Err::<Value, AcpError>(AcpError::new(
                    INTERNAL_ERROR,
                    format!("unknown provider method: {method}"),
                )),
            )));
        }
    };

    let view = serde_json::json!({
        "providers": store.views(),
        "default": catalog.default,
    });

    Ok(JsonRpcMessage::wrap(Response::new(
        id,
        Ok::<_, AcpError>(view),
    )))
}

/// Parses the params of a request, leaving the caller to encode a failure.
fn parse_params<T: serde::de::DeserializeOwned>(
    params: Option<&Value>,
) -> Result<T, serde_json::Error> {
    let value = params.cloned().unwrap_or(Value::Null);
    serde_json::from_value(value)
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
    providers: State<'_, ProviderState>,
    payload: RpcRequest,
) -> Result<RpcReply, String> {
    // Provider management commands are answered straight from the shared
    // store, without going through the agent.
    let method = &payload.inner().method;
    if method.starts_with("_srud/unstable/provider/") {
        return handle_provider_command(&providers.0, &payload).await;
    }

    Ok(state.0.handle(payload).await)
}

/// The working directory new sessions are created with: the directory the
/// app was launched from, falling back to the user's home directory, and to
/// `.` when neither can be found.
#[tauri::command]
fn default_cwd() -> String {
    std::env::current_dir()
        .ok()
        .or_else(config::user_home)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| ".".to_owned())
}

/// An image read from disk, base64-encoded for the prompt.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageFile {
    mime: String,
    data: String,
}

/// Why an image the clipboard offered could not be read.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ImageReadError {
    kind: &'static str,
}

impl ImageReadError {
    fn unreadable() -> Self {
        Self { kind: "unreadable" }
    }

    fn too_large() -> Self {
        Self { kind: "tooLarge" }
    }
}

/// The MIME type an image file's extension names, if it names one.
fn image_mime(path: &std::path::Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Some("image/png"),
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("gif") => Some("image/gif"),
        Some("webp") => Some("image/webp"),
        Some("bmp") => Some("image/bmp"),
        Some("svg") => Some("image/svg+xml"),
        Some("avif") => Some("image/avif"),
        _ => None,
    }
}

/// Whether a path names an image worth reading off the clipboard.
fn is_image_path(path: &std::path::Path) -> bool {
    path.is_absolute() && image_mime(path).is_some()
}

/// Reads an image the clipboard named by path.
///
/// Some Linux clipboards carry a file path rather than the image bytes, and a
/// webview cannot open the file behind a `file://` path on its own. Only image
/// extensions are read, and only up to `limit` bytes, which is the size the
/// composer is willing to send.
#[tauri::command]
fn read_image_file(path: String, limit: u64) -> Result<ImageFile, ImageReadError> {
    let path = std::path::PathBuf::from(path);
    if !path.is_absolute() {
        return Err(ImageReadError::unreadable());
    }
    let Some(mime) = image_mime(&path) else {
        return Err(ImageReadError::unreadable());
    };
    let size = std::fs::metadata(&path)
        .map_err(|_| ImageReadError::unreadable())?
        .len();
    if size > limit {
        return Err(ImageReadError::too_large());
    }
    let bytes = std::fs::read(&path).map_err(|_| ImageReadError::unreadable())?;
    Ok(ImageFile {
        mime: mime.to_owned(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// What the system clipboard holds, as far as the composer can take it.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
enum ClipboardContent {
    /// An image, whichever way the clipboard offered it.
    Image { mime: String, data: String },
    /// Plain text, which the composer writes back into the box itself.
    Text { text: String },
    /// Nothing the composer takes.
    None,
}

/// Reads the system clipboard on the thread that owns it.
///
/// A paste can reach the webview with nothing on it: the platform hands WebKit
/// a file the page may not read, and the default handling then puts the path it
/// was copied from into the box. This is the fallback for that, and it runs on
/// the main thread because GTK's clipboard belongs to it.
#[tauri::command]
async fn read_clipboard(app: AppHandle, limit: u64) -> Result<ClipboardContent, ImageReadError> {
    #[cfg(target_os = "linux")]
    {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = sender.send(linux_clipboard(limit));
        })
        .map_err(|_| ImageReadError::unreadable())?;
        receiver.await.map_err(|_| ImageReadError::unreadable())?
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (app, limit);
        Err(ImageReadError::unreadable())
    }
}

/// Reads the clipboard through GTK, trying the shapes a paste can take.
///
/// A file manager offers the picture as a `file://` URI, an image viewer or a
/// browser offers it as an image target, and everything else is text. Files are
/// tried first because a copied file would otherwise arrive as its path.
#[cfg(target_os = "linux")]
fn linux_clipboard(limit: u64) -> Result<ClipboardContent, ImageReadError> {
    let Some(display) = gtk::gdk::Display::default() else {
        return Ok(ClipboardContent::None);
    };
    let Some(clipboard) = gtk::Clipboard::default(&display) else {
        return Ok(ClipboardContent::None);
    };

    for uri in clipboard.wait_for_uris() {
        let Some(path) = path_of_uri(uri.as_str()) else {
            continue;
        };
        match read_image_file(path, limit) {
            Ok(image) => {
                return Ok(ClipboardContent::Image {
                    mime: image.mime,
                    data: image.data,
                });
            }
            // A file that is merely too big is reported; one that is not an
            // image is left to the text and image targets below.
            Err(error) if error.kind == "tooLarge" => return Err(error),
            Err(_) => continue,
        }
    }

    if clipboard.wait_is_image_available() {
        if let Some(pixbuf) = clipboard.wait_for_image() {
            let bytes = pixbuf
                .save_to_bufferv("png", &[])
                .map_err(|_| ImageReadError::unreadable())?;
            if bytes.len() as u64 > limit {
                return Err(ImageReadError::too_large());
            }
            return Ok(ClipboardContent::Image {
                mime: "image/png".to_owned(),
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            });
        }
    }

    let Some(text) = clipboard.wait_for_text() else {
        return Ok(ClipboardContent::None);
    };
    if text.is_empty() {
        return Ok(ClipboardContent::None);
    }
    // A path the webview could not see either: read it rather than paste it.
    let path = path_of_uri(&text).unwrap_or_else(|| text.to_string());
    if is_image_path(std::path::Path::new(&path)) {
        let image = read_image_file(path, limit)?;
        return Ok(ClipboardContent::Image {
            mime: image.mime,
            data: image.data,
        });
    }
    Ok(ClipboardContent::Text {
        text: text.to_string(),
    })
}

/// The local path a `file://` URI names, if it names one.
#[cfg(target_os = "linux")]
fn path_of_uri(uri: &str) -> Option<String> {
    let url = url::Url::parse(uri).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    Some(url.to_file_path().ok()?.display().to_string())
}

/// Turns off the webview's spell checker.
///
/// WebKitGTK loads the enchant plugins the system provides and logs a warning
/// for every one that is missing. The app ships no dictionaries of its own, so
/// the checker has nothing to offer beyond that noise.
#[cfg(target_os = "linux")]
fn disable_spell_checking<R: tauri::Runtime>(app: &tauri::App<R>) {
    use tauri::Manager;
    use webkit2gtk::{WebContextExt, WebViewExt};

    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let _ = window.with_webview(|webview| {
        if let Some(context) = webview.inner().context() {
            context.set_spell_checking_enabled(false);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Reported, not fatal: the app still runs without it, and the first session
    // that needs a workspace will say so itself.
    if let Err(error) = config::ensure() {
        eprintln!("{error}");
    }
    let app = tauri::Builder::default()
        // Links in assistant output are opened in the user's own browser. Without
        // this a click navigates the webview itself, replacing the whole app with a
        // web page the reader then has to navigate back out of.
        //
        // Its `default` permission set scopes URLs to `http`, `https`, `mailto` and
        // `tel`, so the check is enforced here rather than only in the webview —
        // which matters because the text being rendered is model output.
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // One store shared by the panel commands and the agent: an edit
            // lands in the same lock the next session resolves against, so a
            // provider added at runtime takes effect without a restart.
            let store = match ProviderStore::load() {
                Ok(store) => Arc::new(store),
                Err(err) => {
                    eprintln!(
                        "providers.json could not be read ({err}); starting from an empty store"
                    );
                    let path = config::providers_path()
                        .unwrap_or_else(|_| std::env::temp_dir().join(config::PROVIDERS_FILE));
                    Arc::new(ProviderStore::empty(path))
                }
            };
            app.manage(ProviderState(Arc::clone(&store)));

            let agent = build_agent(store);
            spawn_notify_pump(app.handle().clone(), &agent);
            app.manage(AgentState(agent));
            app.manage(ShutdownState::default());
            #[cfg(target_os = "linux")]
            disable_spell_checking(app);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            rpc_request,
            default_cwd,
            read_image_file,
            read_clipboard
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        let tauri::RunEvent::ExitRequested { api, .. } = event else {
            return;
        };

        let state = app_handle.state::<ShutdownState>();
        match state.0.load(Ordering::Acquire) {
            SHUTDOWN_RUNNING => {
                state.0.store(SHUTDOWN_DRAINING, Ordering::Release);
                api.prevent_exit();

                let app = app_handle.clone();
                let agent = Arc::clone(&app.state::<AgentState>().0);
                tauri::async_runtime::spawn(async move {
                    let settled = agent.shutdown(SHUTDOWN_TIMEOUT).await;
                    if !settled {
                        eprintln!(
                            "agent turns did not settle within {SHUTDOWN_TIMEOUT:?}; exiting anyway"
                        );
                    }
                    app.state::<ShutdownState>()
                        .0
                        .store(SHUTDOWN_READY, Ordering::Release);
                    app.exit(0);
                });
            }
            // A second close while draining must not cut the wait short.
            SHUTDOWN_DRAINING => api.prevent_exit(),
            // READY: the drain has finished and asked for the exit.
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::stream;
    use serde_json::json;
    use srud_core::client::{ModelEvent, ModelRequest, ModelStream};
    use srud_core::tools::ToolRegistry;
    use srud_protocol::acp::{Request, RequestId};
    use srud_server::Model;
    use tokio_util::sync::CancellationToken;

    /// Holds the process environment for one test, restoring it afterwards.
    ///
    /// The configuration directory is read from `SRUD_HOME`, a process-wide
    /// variable shared by every test in the binary, so a test that changes it
    /// takes this first.
    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn take(names: &[&'static str]) -> Self {
            static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
            let lock = LOCK
                .get_or_init(|| std::sync::Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let saved = names
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            Self { _lock: lock, saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    /// Takes the environment and points the configuration directory at a
    /// temporary place of this test's own.
    ///
    /// The agent settles a session's workspace from the configuration directory,
    /// so a test must not let it write into the developer's home. `extra` names
    /// further variables to hold for the length of the test.
    fn home(prefix: &str, extra: &[&'static str]) -> (EnvGuard, std::path::PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let mut names = vec![config::HOME_VAR];
        names.extend_from_slice(extra);
        let guard = EnvGuard::take(&names);
        let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("srud-desktop-{prefix}-{unique}"));
        std::fs::create_dir_all(&dir).expect("a temporary configuration directory");
        std::env::set_var(config::HOME_VAR, &dir);
        (guard, dir)
    }

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
    /// The caller must already hold the configuration directory — see [`home`] —
    /// because the agent settles the session's workspace from it.
    async fn open_session(agent: &Agent) -> String {
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
        let (_env, _dir) = home("notify", &[]);
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
    fn a_provider_becomes_a_client_aimed_at_its_own_endpoint() {
        let provider = Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            base_url: "https://api.example/v1".into(),
            api_key: "sk-test-1234".into(),
            models: vec![Model {
                id: "gpt-4o".into(),
                name: None,
                modalities: vec![],
                context_window: None,
                thinking: None,
            }],
        };
        // No network call: building the client is all this shell decides.
        let model = &provider.models[0];
        assert!(ChatFactory.build(&provider, model, None).is_ok());
    }

    #[tokio::test]
    async fn a_provider_file_makes_the_agent_configurable() {
        let (_env, dir) = home("providers", &[]);
        std::fs::write(
            dir.join("providers.json"),
            serde_json::to_vec(&json!({
                "version": 1,
                "providers": [{
                    "id": "openai",
                    "name": "OpenAI",
                    "baseUrl": "https://api.example/v1",
                    "apiKey": "sk-test-1234",
                    "models": [{ "id": "gpt-4o", "name": "GPT-4o" }],
                }],
            }))
            .unwrap(),
        )
        .unwrap();

        let store = Arc::new(ProviderStore::load().expect("provider store"));
        let agent = build_agent(store);
        agent
            .handle(req(
                "initialize",
                json!({ "protocolVersion": 1, "clientCapabilities": {} }),
            ))
            .await;
        let reply = agent
            .handle(req("_srud/unstable/provider/list", json!({})))
            .await;

        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("provider/list failed");
        };
        assert_eq!(result["providers"][0]["id"], json!("openai"));
        // The key travels back masked, never whole.
        assert_eq!(result["providers"][0]["apiKey"], json!("sk-…1234"));
    }

    #[tokio::test]
    async fn an_empty_store_still_answers_and_asks_for_a_model() {
        let (_env, _dir) = home("empty", &[]);
        let store = Arc::new(ProviderStore::load().expect("provider store"));
        let agent = build_agent(store);

        let session_id = open_session(&agent).await;
        let reply = agent
            .handle(req(
                "session/prompt",
                json!({
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": "hi" }],
                }),
            ))
            .await;

        let Response::Error { error, .. } = reply.into_inner() else {
            panic!("a prompt with no model configured must fail");
        };
        assert_eq!(
            i32::from(error.code),
            srud_protocol::error::INVALID_PARAMS,
            "the session is told to choose a model, not left without an answer"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_file_uri_becomes_a_decoded_local_path() {
        assert_eq!(
            path_of_uri("file:///home/me/a%20shot.png").as_deref(),
            Some("/home/me/a shot.png")
        );
        assert_eq!(path_of_uri("https://example.test/a.png"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn only_an_absolute_image_path_is_read_off_the_clipboard() {
        assert!(is_image_path(std::path::Path::new("/home/me/a.PNG")));
        assert!(!is_image_path(std::path::Path::new("/home/me/notes.txt")));
        assert!(!is_image_path(std::path::Path::new("a.png")));
    }

    /// The real end-to-end path with live model credentials from the
    /// environment (`SRUD_API_KEY` and friends): webview request → agent →
    /// core → provider → streamed `rpc_notify` payloads. Run explicitly:
    ///
    /// ```sh
    /// cargo test -p desktop -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore = "requires live model credentials in the environment"]
    async fn a_prompt_round_trip_streams_live_text_to_the_webview() {
        let (_env, _dir) = home("live", &[]);
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
