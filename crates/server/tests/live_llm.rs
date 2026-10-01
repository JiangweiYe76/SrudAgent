//! End-to-end check against a live model provider.
//!
//! Exercises the exact ACP RPC sequence the desktop shell drives —
//! `initialize` → `session/new` → `session/prompt` — through the real
//! `ChatClient`, and asserts the turn streams assistant text and returns a
//! stop reason. Requires model credentials in the environment; run it with:
//!
//! ```sh
//! cargo test -p srud-server --test live_llm -- --ignored --nocapture
//! ```

use std::sync::Arc;

use serde_json::{json, Value};
use srud_client::openai::ChatClient;
use srud_core::client::ModelClient;
use srud_core::tools::ToolRegistry;
use srud_protocol::acp::{JsonRpcMessage, Notification, Request, RequestId, Response, SessionId};
use srud_server::Agent;

/// Loads `.env.test` from the workspace root, walking up from this crate.
fn load_env() {
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let candidate = dir.join(".env.test");
        if candidate.exists() {
            let _ = dotenvy::from_path(&candidate);
            return;
        }
        if !dir.pop() {
            return;
        }
    }
}

fn request(method: &str, params: Value) -> JsonRpcMessage<Request<Value>> {
    JsonRpcMessage::wrap(Request {
        id: RequestId::Number(1),
        method: method.into(),
        params: Some(params),
    })
}

#[tokio::test]
#[ignore = "requires live model credentials; run explicitly with --ignored"]
async fn a_prompt_round_trip_streams_an_llm_response() {
    load_env();
    let Ok(model) = ChatClient::from_env() else {
        eprintln!("SRUD_API_KEY not set; skipping live test");
        return;
    };
    let model: Arc<dyn ModelClient> = Arc::new(model);
    let agent = Agent::new(model, Arc::new(ToolRegistry::new()));

    let reply = agent
        .handle(request(
            "initialize",
            json!({ "protocolVersion": 1, "clientCapabilities": {} }),
        ))
        .await;
    assert!(
        matches!(reply.inner(), Response::Result { .. }),
        "initialize failed: {reply:?}"
    );

    let reply = agent
        .handle(request(
            "session/new",
            json!({ "cwd": "/tmp", "mcpServers": [] }),
        ))
        .await;
    let Response::Result { result, .. } = reply.into_inner() else {
        panic!("session/new failed");
    };
    let session_id = SessionId::new(result["sessionId"].as_str().expect("sessionId").to_string());

    // Subscribe before prompting so every streamed update is buffered.
    let mut rx = agent.subscribe();
    let reply = agent
        .handle(request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{ "type": "text", "text": "Reply with exactly: SRUD-OK" }],
            }),
        ))
        .await;
    let Response::Result { result, .. } = reply.into_inner() else {
        panic!("session/prompt failed");
    };
    let stop_reason = result["stopReason"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Drain the buffered notifications and reassemble the assistant text.
    // `try_recv` is synchronous: the turn has ended, so only what the
    // channel already holds is read, then it reports empty and we stop.
    let mut streamed = String::new();
    let mut updates = 0usize;
    while let Ok(Notification { params, .. }) = rx.try_recv() {
        let Some(params) = params else { continue };
        let value = serde_json::to_value(&params).expect("serializable");
        let Some(update) = value.get("update") else {
            continue;
        };
        updates += 1;
        if update["sessionUpdate"].as_str() == Some("agent_message_chunk") {
            if let Some(text) = update["content"]["text"].as_str() {
                streamed.push_str(text);
            }
        }
    }

    eprintln!("updates={updates} stopReason={stop_reason} text={streamed:?}");
    assert!(
        !streamed.trim().is_empty(),
        "no assistant text was streamed"
    );
    assert_eq!(stop_reason, "end_turn", "turn should complete normally");
}
