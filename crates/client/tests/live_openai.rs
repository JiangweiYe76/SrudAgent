//! Live tests against a real OpenAI-compatible endpoint, over both the
//! Responses API and Chat Completions.
//!
//! These are ignored by default so `cargo test` never needs a network or a key.
//! To run them, put your credentials in a `.env.test` file at the repository
//! root (or in this crate's directory):
//!
//! ```text
//! SRUD_BASE_URL=https://api.openai.com/v1
//! SRUD_API_KEY=sk-...
//! SRUD_MODEL=gpt-4o-mini
//! ```
//!
//! `SRUD_BASE_URL` must include the version segment the provider expects. Both
//! endpoints are requested from it: `<base>/responses` and
//! `<base>/chat/completions`. `SRUD_MODEL` defaults to `gpt-5` when omitted.
//!
//! Then run:
//!
//! ```text
//! cargo test -p srud-client --test live_openai -- --ignored --nocapture
//! ```
//!
//! If no credentials are found the tests report what is missing and pass.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use srud_client::openai::{ChatClient, ClientConfig, ResponsesClient};
use srud_core::client::ModelClient;
use srud_core::session::Session;
use srud_core::tools::{Tool, ToolError, ToolOutcome, ToolRegistry};
use srud_core::types::{Event, EventSink, ResponseItem, TurnEndReason, TurnInput};

/// What the tool returns; the model has to relay it back for the round trip to
/// be proven.
const SECRET: &str = "ZQ-4417";

const PLAIN_PROMPT: &str = "Reply with the single word: pong";
const TOOL_PROMPT: &str = "Call the get_secret_code tool, then tell me the code it returned.";
const THINKING_PROMPT: &str = "Is 17 * 23 larger than 391? Work it out before answering.";

/// Records events so the test can assert on what happened.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
}

impl EventSink for Recorder {
    fn emit(&self, event: Event) {
        self.events.lock().expect("recorder lock").push(event);
    }
}

impl Recorder {
    fn take(&self) -> Vec<Event> {
        self.events.lock().expect("recorder lock").clone()
    }
}

/// Returns a fixed code.
struct SecretCode;

#[async_trait::async_trait]
impl Tool for SecretCode {
    fn name(&self) -> &str {
        "get_secret_code"
    }
    fn description(&self) -> &str {
        "Returns the secret code. Use this whenever the code itself is asked for."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }
    async fn call(&self, _arguments: serde_json::Value) -> Result<ToolOutcome, ToolError> {
        Ok(ToolOutcome::success(SECRET))
    }
}

/// What one live turn produced.
struct Outcome {
    reason: TurnEndReason,
    events: Vec<Event>,
    history: Vec<ResponseItem>,
}

impl Outcome {
    /// The assistant-visible text, reassembled from its deltas.
    fn reply(&self) -> String {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::AgentMessageDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The reasoning text, reassembled from its deltas.
    fn thought(&self) -> String {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::AgentThoughtDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Position of the first event matching a predicate.
    fn first_index(&self, wanted: impl Fn(&Event) -> bool) -> Option<usize> {
        self.events.iter().position(wanted)
    }

    /// Names of the tools that ran.
    fn tools_called(&self) -> Vec<&str> {
        self.events
            .iter()
            .filter_map(|event| match event {
                Event::ToolCallBegin { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect()
    }
}

/// Loads `.env.test`, preferring this crate's directory over the crate root.
fn load_env_test() -> Option<PathBuf> {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        crate_dir.join(".env.test"),
        crate_dir.join("..").join("..").join(".env.test"),
    ];

    for path in candidates {
        if path.is_file() {
            if let Err(error) = dotenvy::from_path(&path) {
                eprintln!("could not read {}: {error}", path.display());
            }
            return Some(path);
        }
    }
    None
}

/// Reads settings, or explains why the test is skipped.
fn live_settings() -> Option<ClientConfig> {
    let loaded = load_env_test();

    let settings = match ClientConfig::from_env() {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("skipping live test: {error}");
            eprintln!(
                "put {} / {} / {} in .env.test at the repository root",
                ClientConfig::BASE_URL_VAR,
                ClientConfig::API_KEY_VAR,
                ClientConfig::MODEL_VAR
            );
            return None;
        }
    };

    eprintln!(
        "live endpoint: {} (model {}), settings from {}",
        settings.base_url.as_deref().unwrap_or("<sdk default>"),
        settings.model,
        loaded
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "the process environment".to_owned())
    );

    Some(settings)
}

/// Builds both clients from one set of settings.
fn clients() -> Option<(ResponsesClient, ChatClient)> {
    let settings = live_settings()?;
    Some((
        ResponsesClient::new(settings.clone()),
        ChatClient::new(settings),
    ))
}

/// Runs one turn, optionally with the code tool registered.
async fn run(client: &dyn ModelClient, text: &str, with_tool: bool) -> Outcome {
    let session = Session::new();
    let sink = Recorder::default();
    let mut tools = ToolRegistry::new();
    if with_tool {
        assert!(tools.register(Arc::new(SecretCode)).is_ok());
    }

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: text.into() },
        client,
        &tools,
        &sink,
    )
    .await
    .expect("the turn completes");

    let history = {
        let state = session.state();
        state.history().to_vec()
    };

    Outcome {
        reason: result.reason,
        events: sink.take(),
        history,
    }
}

/// Asserts a plain exchange produced text.
fn assert_plain_exchange(outcome: &Outcome, label: &str) {
    let reply = outcome.reply();
    eprintln!("{label} reply: {reply:?}");
    assert_eq!(outcome.reason, TurnEndReason::Completed, "{label}");
    assert!(
        !reply.trim().is_empty(),
        "{label}: the model produced no text"
    );
}

/// Asserts the tool ran once, its output was recorded, and the model relayed it.
fn assert_tool_round_trip(outcome: &Outcome, label: &str) {
    let reply = outcome.reply();
    eprintln!("{label} reply: {reply:?}");

    assert_eq!(
        outcome.tools_called(),
        ["get_secret_code"],
        "{label}: the tool runs exactly once"
    );

    assert!(
        outcome.events.iter().any(|event| matches!(
            event,
            Event::ToolCallEnd { result, .. } if !result.is_error && result.output == SECRET
        )),
        "{label}: the tool reported its code"
    );

    assert!(
        outcome.history.iter().any(|item| matches!(
            item,
            ResponseItem::FunctionCallOutput { output, .. } if output == SECRET
        )),
        "{label}: the tool output was recorded for the follow-up request"
    );

    assert_eq!(outcome.reason, TurnEndReason::Completed, "{label}");
    assert!(
        reply.contains(SECRET),
        "{label}: the model did not relay the code back: {reply:?}"
    );
}

#[tokio::test]
#[ignore = "requires a live endpoint and credentials"]
async fn responses_answers_a_plain_prompt() {
    let Some((responses, _)) = clients() else {
        return;
    };
    let outcome = run(&responses, PLAIN_PROMPT, false).await;
    assert_plain_exchange(&outcome, "responses");
}

#[tokio::test]
#[ignore = "requires a live endpoint and credentials"]
async fn responses_round_trips_a_tool_call() {
    let Some((responses, _)) = clients() else {
        return;
    };
    let outcome = run(&responses, TOOL_PROMPT, true).await;
    assert_tool_round_trip(&outcome, "responses");
}

#[tokio::test]
#[ignore = "requires a live endpoint and credentials"]
async fn chat_completions_answers_a_plain_prompt() {
    let Some((_, chat)) = clients() else {
        return;
    };
    let outcome = run(&chat, PLAIN_PROMPT, false).await;
    assert_plain_exchange(&outcome, "chat completions");
}

#[tokio::test]
#[ignore = "requires a live endpoint and credentials"]
async fn chat_completions_round_trips_a_tool_call() {
    let Some((_, chat)) = clients() else {
        return;
    };
    let outcome = run(&chat, TOOL_PROMPT, true).await;
    assert_tool_round_trip(&outcome, "chat completions");
}

/// Requires a model that exposes its reasoning — a provider that streams
/// `reasoning_content`. Such a model makes this test prove the mapping;
/// without one it reports a skip, because that is a property of the endpoint
/// rather than of this crate.
#[tokio::test]
#[ignore = "requires a live endpoint and credentials"]
async fn chat_completions_streams_reasoning_as_thoughts() {
    let Some((_, chat)) = clients() else {
        return;
    };
    let outcome = run(&chat, THINKING_PROMPT, false).await;
    assert_eq!(outcome.reason, TurnEndReason::Completed);

    let thought = outcome.thought();
    if thought.trim().is_empty() {
        eprintln!("skipping: the configured model exposed no reasoning");
        return;
    }
    eprintln!("thought ({} chars): {:.160}", thought.len(), thought);

    assert!(
        outcome
            .history
            .iter()
            .any(|item| matches!(item, ResponseItem::Reasoning { .. })),
        "the reasoning was not recorded in the session"
    );

    let first_thought = outcome
        .first_index(|event| matches!(event, Event::AgentThoughtDelta { .. }))
        .expect("reasoning arrived, so a thought delta exists");
    let first_text = outcome
        .first_index(|event| matches!(event, Event::AgentMessageDelta { .. }))
        .expect("the answer arrived as text");
    assert!(
        first_thought < first_text,
        "reasoning must precede the answer it produced"
    );
    assert!(
        !outcome.reply().trim().is_empty(),
        "the model produced no answer after reasoning"
    );
}
