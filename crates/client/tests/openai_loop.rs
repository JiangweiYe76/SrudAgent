//! End-to-end test: the core's turn loop driving a real client whose endpoint
//! cannot be reached, so the request fails locally while the SDK's request
//! building and our translation still run for real.

use std::sync::{Arc, Mutex};

use srud_client::openai::{ChatClient, ClientConfig, ResponsesClient};
use srud_core::client::ModelClient;
use srud_core::session::Session;
use srud_core::session_log::Volatile;
use srud_core::tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolRegistry};
use srud_core::types::{Event, EventSink, ResponseItem, Role, TurnEndReason, TurnInput};

/// Records events so the test can assert on order.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
}

impl EventSink for Recorder {
    fn emit(&self, event: Event) {
        self.events.lock().expect("recorder lock").push(event);
    }
}

/// A tool that returns a fixed string.
struct Static;

#[async_trait::async_trait]
impl Tool for Static {
    fn name(&self) -> &str {
        "read_file"
    }
    fn description(&self) -> &str {
        "Reads a file."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        Ok(ToolOutcome::success("file contents"))
    }
}

/// Settings pointed at a port nothing listens on.
fn unreachable() -> ClientConfig {
    ClientConfig::new("test-key", "gpt-x").with_base_url("http://127.0.0.1:1")
}

/// Runs a turn against a client that cannot connect, asserting the loop reports
/// the failure instead of panicking.
async fn assert_fails_cleanly(client: &dyn ModelClient) {
    let session = Session::new("workspace");
    let sink = Recorder::default();
    let mut tools = ToolRegistry::new();
    assert!(tools.register(Arc::new(Static)).is_ok());

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput {
            text: "read the file".into(),
        },
        client,
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("the turn completes even when the request cannot be made");

    assert_eq!(result.reason, TurnEndReason::Error);

    let events = sink.events.lock().expect("recorder lock").clone();
    let terminators = events
        .iter()
        .filter(|event| matches!(event, Event::TurnComplete { .. }))
        .count();
    assert_eq!(terminators, 1, "exactly one terminator");

    // The env-context block leads, then the user's input; both are recorded
    // before the failure.
    let state = session.state();
    assert!(srud_core::context::is_item(&state.history()[0]));
    assert!(matches!(
        &state.history()[1],
        ResponseItem::Message { role: Role::User, content } if content == "read the file"
    ));
}

#[test]
fn both_clients_expose_the_configured_model() {
    assert_eq!(ResponsesClient::new(unreachable()).model(), "gpt-x");
    assert_eq!(ChatClient::new(unreachable()).model(), "gpt-x");
}

#[tokio::test]
async fn the_loop_survives_a_responses_endpoint_it_cannot_reach() {
    assert_fails_cleanly(&ResponsesClient::new(unreachable())).await;
}

#[tokio::test]
async fn the_loop_survives_a_chat_endpoint_it_cannot_reach() {
    assert_fails_cleanly(&ChatClient::new(unreachable())).await;
}
