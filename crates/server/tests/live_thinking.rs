//! End-to-end check of a configured thinking model against a live endpoint.
//!
//! Loads a real `providers.json` into a temporary configuration directory,
//! selects the model through ACP's own config option, changes its thinking
//! level, and runs one turn through the real `ChatClient`.
//!
//! Run it with:
//!
//! ```sh
//! SRUD_LIVE_PROVIDERS="$HOME/.srudagent/providers.json" \
//! SRUD_LIVE_MODEL="bailian/kimi-k3" \
//! cargo test -p srud-server --test live_thinking -- --ignored --nocapture
//! ```

use std::env;
use std::fs;
use std::sync::Arc;

use serde_json::{json, Value};
use srud_client::openai::{ChatClient, ClientConfig};
use srud_core::client::{ModelClient, ModelError};
use srud_core::thinking::ThinkingRequest;
use srud_core::tools::ToolRegistry;
use srud_protocol::acp::{JsonRpcMessage, Notification, Request, RequestId, Response, SessionId};
use srud_server::{Agent, Model, ModelFactory, ModelProviders, Provider, ProviderStore};

struct Factory;

impl ModelFactory for Factory {
    fn build(
        &self,
        provider: &Provider,
        model: &Model,
        thinking: Option<ThinkingRequest>,
    ) -> Result<Arc<dyn ModelClient>, ModelError> {
        let settings = ClientConfig::new(provider.api_key.clone(), model.id.clone())
            .with_base_url(provider.base_url.clone())
            .with_thinking(thinking);
        Ok(Arc::new(ChatClient::new(settings)))
    }
}

fn request(method: &str, params: Value) -> JsonRpcMessage<Request<Value>> {
    JsonRpcMessage::wrap(Request {
        id: RequestId::Number(1),
        method: method.into(),
        params: Some(params),
    })
}

async fn result_of(agent: &Agent, method: &str, params: Value) -> Value {
    let reply = agent.handle(request(method, params)).await;
    match reply.into_inner() {
        Response::Result { result, .. } => result,
        other => panic!("{method} failed: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires a real providers.json with live credentials; run explicitly with --ignored"]
async fn a_configured_thinking_model_answers() {
    let Ok(source) = env::var("SRUD_LIVE_PROVIDERS") else {
        eprintln!("SRUD_LIVE_PROVIDERS is not set; skipping live test");
        return;
    };
    let home = env::temp_dir().join(format!("srud-live-thinking-{}", std::process::id()));
    fs::create_dir_all(&home).expect("a temporary configuration directory");
    fs::copy(&source, home.join("providers.json")).expect("the provider file is readable");
    env::set_var("SRUD_HOME", &home);

    let store = Arc::new(ProviderStore::load().expect("a provider file this build reads"));
    let agent = Agent::with_providers(
        None,
        Arc::new(ToolRegistry::new()),
        ModelProviders::new(store, Arc::new(Factory)),
    );

    result_of(
        &agent,
        "initialize",
        json!({ "protocolVersion": 1, "clientCapabilities": {} }),
    )
    .await;

    let result = result_of(
        &agent,
        "session/new",
        json!({ "cwd": "", "mcpServers": [] }),
    )
    .await;
    let session_id = SessionId::new(result["sessionId"].as_str().expect("a session").to_string());
    let reference = env::var("SRUD_LIVE_MODEL").unwrap_or_else(|_| "bailian/kimi-k3".into());

    let chosen = result_of(
        &agent,
        "session/set_config_option",
        json!({
            "sessionId": session_id.0.as_ref(),
            "configId": "model",
            "value": reference,
        }),
    )
    .await;
    let thinking = chosen["configOptions"]
        .as_array()
        .expect("config options")
        .iter()
        .find(|option| option["id"] == json!("reasoning"))
        .cloned()
        .expect("the model declares thinking");
    eprintln!("thinking option: {thinking}");

    let levels = thinking["options"].as_array().expect("level values");
    let preferred = env::var("SRUD_LIVE_LEVEL").ok();
    let level = preferred
        .as_deref()
        .and_then(|preferred| {
            levels
                .iter()
                .find(|level| level["value"] == json!(preferred))
        })
        .or_else(|| levels.iter().find(|level| level["value"] == json!("max")))
        .or_else(|| levels.last())
        .expect("at least one level")["value"]
        .as_str()
        .expect("a level id")
        .to_owned();

    let changed = result_of(
        &agent,
        "session/set_config_option",
        json!({
            "sessionId": session_id.0.as_ref(),
            "configId": "reasoning",
            "value": level,
        }),
    )
    .await;
    assert_eq!(
        changed["configOptions"]
            .as_array()
            .expect("config options")
            .iter()
            .find(|option| option["id"] == json!("reasoning"))
            .expect("the thinking option")["currentValue"],
        json!(level)
    );

    let mut rx = agent.subscribe();
    let result = result_of(
        &agent,
        "session/prompt",
        json!({
            "sessionId": session_id.0.as_ref(),
            "prompt": [{ "type": "text", "text": "Reply with exactly: SRUD-OK" }],
        }),
    )
    .await;
    assert_eq!(result["stopReason"], json!("end_turn"));

    let mut streamed = String::new();
    let mut thoughts = 0usize;
    while let Ok(Notification { params, .. }) = rx.try_recv() {
        let Some(params) = params else { continue };
        let value = serde_json::to_value(&params).expect("serializable");
        let Some(update) = value.get("update") else {
            continue;
        };
        match update["sessionUpdate"].as_str() {
            Some("agent_message_chunk") => {
                if let Some(text) = update["content"]["text"].as_str() {
                    streamed.push_str(text);
                }
            }
            Some("agent_thought_chunk") => thoughts += 1,
            _ => {}
        }
    }
    eprintln!("thinking={level} thoughts={thoughts} text={streamed:?}");
    assert!(
        !streamed.trim().is_empty(),
        "no assistant text was streamed"
    );
}
