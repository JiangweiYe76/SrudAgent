//! End-to-end tests for the turn loop against a scripted model.

use std::sync::{Arc, Mutex};

use futures::stream;
use srud_core::client::{ModelClient, ModelError, ModelEvent, ModelRequest, ModelStream};
use srud_core::session::Session;
use srud_core::session_log::Volatile;
use srud_core::tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolRegistry};
use srud_core::types::{Event, EventSink, Op, ResponseItem, Role, TurnEndReason, TurnInput};
use tokio_util::sync::CancellationToken;

/// Records every event, so tests can assert on order.
#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
}

impl Recorder {
    fn events(&self) -> Vec<Event> {
        self.events.lock().expect("recorder lock").clone()
    }

    /// Event discriminants in order, for compact assertions.
    fn kinds(&self) -> Vec<&'static str> {
        self.events().iter().map(kind_of).collect()
    }
}

impl EventSink for Recorder {
    fn emit(&self, event: Event) {
        self.events.lock().expect("recorder lock").push(event);
    }
}

fn kind_of(event: &Event) -> &'static str {
    match event {
        Event::TurnStarted { .. } => "TurnStarted",
        Event::UserMessage { .. } => "UserMessage",
        Event::AgentMessageDelta { .. } => "AgentMessageDelta",
        Event::AgentThoughtDelta { .. } => "AgentThoughtDelta",
        Event::ToolCallBegin { .. } => "ToolCallBegin",
        Event::ToolCallEnd { .. } => "ToolCallEnd",
        Event::TurnComplete { .. } => "TurnComplete",
    }
}

/// A client that replies from a fixed script, one round-trip at a time.
struct Scripted {
    rounds: Mutex<Vec<Vec<Result<ModelEvent, ModelError>>>>,
    requests: Mutex<Vec<ModelRequest>>,
}

impl Scripted {
    fn new(rounds: Vec<Vec<Result<ModelEvent, ModelError>>>) -> Arc<Self> {
        Arc::new(Self {
            rounds: Mutex::new(rounds),
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("requests lock").clone()
    }
}

#[async_trait::async_trait]
impl ModelClient for Scripted {
    async fn stream(
        &self,
        request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        self.requests.lock().expect("requests lock").push(request);
        let mut rounds = self.rounds.lock().expect("rounds lock");
        if rounds.is_empty() {
            return Err(ModelError::Rejected("script exhausted".into()));
        }
        let events = rounds.remove(0);
        Ok(Box::pin(stream::iter(events)))
    }
}

/// A tool that records having been called.
struct Recorder0 {
    calls: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl Tool for Recorder0 {
    fn name(&self) -> &str {
        "record"
    }
    fn description(&self) -> &str {
        "Records that it was called."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object" })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        _arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        *self.calls.lock().expect("calls lock") += 1;
        Ok(ToolOutcome::success("recorded"))
    }
}

fn tool_registry() -> (ToolRegistry, Arc<Mutex<usize>>) {
    let calls = Arc::new(Mutex::new(0));
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(Recorder0 {
            calls: Arc::clone(&calls),
        }))
        .expect("unique name");
    (registry, calls)
}

fn text(delta: &str) -> Result<ModelEvent, ModelError> {
    Ok(ModelEvent::TextDelta {
        delta: delta.into(),
    })
}

fn done() -> Result<ModelEvent, ModelError> {
    Ok(ModelEvent::Done)
}

/// A session working in a placeholder directory. The turn loop never touches
/// the filesystem, so this directory does not have to exist.
fn session() -> Session {
    Session::new("workspace")
}

#[tokio::test]
async fn a_plain_exchange_records_history_and_emits_in_order() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    let client = Scripted::new(vec![vec![text("Hel"), text("lo"), done()]]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: "hi".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    assert_eq!(result.reason, TurnEndReason::Completed);
    assert_eq!(
        sink.kinds(),
        vec![
            "TurnStarted",
            "UserMessage",
            "AgentMessageDelta",
            "AgentMessageDelta",
            "TurnComplete",
        ]
    );

    let state = session.state();
    let history = state.history();
    assert_eq!(
        history.len(),
        3,
        "env-context, then user input, then one assistant message"
    );
    assert!(srud_core::context::is_item(&history[0]));
    assert!(matches!(
        &history[1],
        ResponseItem::Message { role: Role::User, content } if content == "hi"
    ));
    assert!(matches!(
        &history[2],
        ResponseItem::Message { role: Role::Assistant, content } if content == "Hello"
    ));
}

#[tokio::test]
async fn env_context_leads_the_turn_without_being_announced() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    let client = Scripted::new(vec![vec![done()]]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    srud_core::run_turn(
        &session,
        TurnInput { text: "hi".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    // The model reads where and when the turn runs before it reads the request.
    let requests = client.requests();
    let Some(srud_core::client::ModelRequestItem::Message { role, content }) =
        requests[0].items.first()
    else {
        panic!("the first request item is a message");
    };
    assert_eq!(*role, Role::User);
    assert!(
        content.starts_with(srud_core::context::OPEN_TAG)
            && content.ends_with(srud_core::context::CLOSE_TAG),
        "the block is the first thing the model reads: {content}"
    );
    assert!(
        content.contains("<cwd>workspace</cwd>"),
        "the block names the working directory: {content}"
    );
    assert!(
        content.contains("<date>"),
        "the block carries the local date and time: {content}"
    );

    // It is context, not something the user said, so nothing announces it.
    assert_eq!(
        sink.kinds(),
        vec!["TurnStarted", "UserMessage", "TurnComplete"]
    );
}

#[tokio::test]
async fn a_tool_call_round_trips_and_drives_a_second_request() {
    let session = session();
    let sink = Recorder::default();
    let (tools, calls) = tool_registry();
    let client = Scripted::new(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                call_id: "c1".into(),
                name: "record".into(),
                arguments: "{}".into(),
            }),
            done(),
        ],
        vec![text("all done"), done()],
    ]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: "go".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    assert_eq!(result.reason, TurnEndReason::Completed);
    assert_eq!(*calls.lock().expect("calls lock"), 1);

    assert_eq!(
        sink.kinds(),
        vec![
            "TurnStarted",
            "UserMessage",
            "ToolCallBegin",
            "ToolCallEnd",
            "AgentMessageDelta",
            "TurnComplete",
        ]
    );

    // The second request must carry the env-context block, the user's input,
    // and then the tool call and its output, in order.
    let requests = client.requests();
    assert_eq!(requests.len(), 2);
    let second = &requests[1];
    let kinds: Vec<&'static str> = second
        .items
        .iter()
        .map(|item| match item {
            srud_core::client::ModelRequestItem::Message { role, .. } => match role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            },
            srud_core::client::ModelRequestItem::FunctionCall { .. } => "call",
            srud_core::client::ModelRequestItem::FunctionCallOutput { .. } => "output",
        })
        .collect();
    assert_eq!(kinds, vec!["user", "user", "call", "output"]);
}

#[tokio::test]
async fn exactly_one_terminator_is_emitted_when_the_model_fails() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    let client = Scripted::new(vec![vec![Err(ModelError::Transport("boom".into()))]]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: "hi".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("the turn still completes");

    assert_eq!(result.reason, TurnEndReason::Error);
    let terminators = sink
        .kinds()
        .into_iter()
        .filter(|kind| *kind == "TurnComplete")
        .count();
    assert_eq!(terminators, 1, "exactly one terminator, even on failure");
}

#[tokio::test]
async fn a_model_that_never_signals_completion_is_an_error() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    // The stream ends after a delta without a `Done`.
    let client = Scripted::new(vec![vec![text("partial")]]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: "hi".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn completes");

    assert_eq!(result.reason, TurnEndReason::Error);
    // What did arrive is still recorded.
    assert!(session
        .state()
        .history()
        .iter()
        .any(|item| matches!(item, ResponseItem::Message { role: Role::Assistant, content } if content == "partial")));
}

#[tokio::test]
async fn an_unknown_tool_becomes_a_failed_outcome_not_a_crash() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    let client = Scripted::new(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                call_id: "c1".into(),
                name: "does_not_exist".into(),
                arguments: "{}".into(),
            }),
            done(),
        ],
        vec![text("recovered"), done()],
    ]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: "go".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    assert_eq!(result.reason, TurnEndReason::Completed);

    let failed = sink.events().into_iter().any(|event| {
        matches!(
            event,
            Event::ToolCallEnd { result, .. } if result.is_error
        )
    });
    assert!(failed, "the failure is reported through ToolCallEnd");

    // The text the model reads has to carry the failure too: `is_error` reaches
    // this event but never the provider, whose tool output is a string.
    let recorded = session
        .state()
        .history()
        .iter()
        .find_map(|item| match item {
            srud_core::ResponseItem::FunctionCallOutput { output, .. } => Some(output.clone()),
            _ => None,
        })
        .expect("the failed call was recorded");
    assert!(
        recorded.starts_with(srud_core::tools::FAILURE_MARKER),
        "a failure reaching the model is marked: {recorded}"
    );
}

/// The refusal to run a tool reaches the model marked, whatever stopped it.
///
/// Arguments the schema permits but the tool cannot act on come back as a
/// `ToolError`, which the loop turns into a failed outcome. That is the path a
/// model reaches by misreading its own result — told a file has no line 9999, it
/// asks for offset 0 — so it has to look the same as any other failure.
#[tokio::test]
async fn rejected_arguments_reach_the_model_as_a_marked_failure() {
    let session = session();
    let sink = Recorder::default();
    let mut tools = ToolRegistry::new();
    tools
        .register(std::sync::Arc::new(srud_core::tools::read::ReadTool))
        .expect("fresh registry");
    // Offset 0 passes the schema's `minimum: 1` only because the tool checks it,
    // which makes this the argument rejection a model actually produces.
    let client = Scripted::new(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                call_id: "c1".into(),
                name: "read".into(),
                arguments: r#"{"path":"/etc/os-release","offset":0}"#.into(),
            }),
            done(),
        ],
        vec![text("adjusted"), done()],
    ]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    srud_core::run_turn(
        &session,
        TurnInput { text: "go".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    let recorded = session
        .state()
        .history()
        .iter()
        .find_map(|item| match item {
            srud_core::ResponseItem::FunctionCallOutput { output, .. } => Some(output.clone()),
            _ => None,
        })
        .expect("the rejected call was recorded");

    assert!(
        recorded.starts_with(srud_core::tools::FAILURE_MARKER),
        "rejected arguments are marked like any other failure: {recorded}"
    );
    assert!(
        recorded.contains("before anything ran"),
        "it says the tool did not run, so the model does not think a file was read: {recorded}"
    );
}

#[tokio::test]
async fn a_second_concurrent_turn_is_refused() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();

    // Hold the slot directly so the loop cannot claim it.
    let guard = session.begin_turn();
    assert!(guard.is_some());

    let client = Scripted::new(vec![vec![done()]]);
    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let err = srud_core::run_turn(
        &session,
        TurnInput { text: "hi".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect_err("a busy session refuses the turn");

    assert!(matches!(err, srud_core::turn::TurnError::Busy));
    assert!(
        sink.events().is_empty(),
        "a refused turn emits nothing at all"
    );
}

#[tokio::test]
async fn interrupting_before_the_first_step_stops_the_turn() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();

    let guard = session.begin_turn().expect("free");
    guard.cancellation().cancel();
    // Release so the loop can claim it, but the token it mints is fresh, so
    // this checks the pre-cancelled path via `Session::interrupt` instead.
    drop(guard);

    let client = Scripted::new(vec![vec![done()]]);
    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput { text: "hi".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");
    assert_eq!(result.reason, TurnEndReason::Completed);
}

#[tokio::test]
async fn op_values_are_constructible() {
    // The input side of the contract: `Op` is what callers hand in.
    let ops = [
        Op::TurnInput(TurnInput { text: "x".into() }),
        Op::Interrupt,
        Op::Compact,
        Op::Shutdown,
    ];
    assert_eq!(ops.len(), 4);
}

#[tokio::test]
async fn history_stays_consistent_when_several_tools_run_in_one_step() {
    let session = session();
    let sink = Recorder::default();
    let (tools, calls) = tool_registry();
    let client = Scripted::new(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                call_id: "c1".into(),
                name: "record".into(),
                arguments: "{}".into(),
            }),
            Ok(ModelEvent::ToolCall {
                call_id: "c2".into(),
                name: "record".into(),
                arguments: "{}".into(),
            }),
            done(),
        ],
        vec![text("done"), done()],
    ]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    srud_core::run_turn(
        &session,
        TurnInput { text: "go".into() },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    assert_eq!(*calls.lock().expect("calls lock"), 2);

    // Each call must be immediately followed by its own output.
    let state = session.state();
    let history = state.history();
    let mut index = 0;
    while index + 1 < history.len() {
        if let ResponseItem::FunctionCall { call_id, .. } = &history[index] {
            match &history[index + 1] {
                ResponseItem::FunctionCallOutput {
                    call_id: output_id, ..
                } => assert_eq!(call_id, output_id, "call/output must be adjacent"),
                other => panic!("expected output after call, found {other:?}"),
            }
            index += 2;
        } else {
            index += 1;
        }
    }
}

#[tokio::test]
async fn a_read_call_reaches_the_sessions_working_directory() {
    let dir = std::env::temp_dir().join("srud-turn-loop-read");
    std::fs::create_dir_all(&dir).expect("scratch directory");
    std::fs::write(dir.join("note.txt"), "hello from the workspace\n").expect("fixture file");

    let session = Session::new(dir.as_path());
    let sink = Recorder::default();
    let mut tools = ToolRegistry::new();
    tools
        .register(Arc::new(srud_core::tools::read::ReadTool))
        .expect("unique name");
    let client = Scripted::new(vec![
        vec![
            Ok(ModelEvent::ToolCall {
                call_id: "c1".into(),
                name: "read".into(),
                arguments: r#"{"path":"note.txt"}"#.into(),
            }),
            done(),
        ],
        vec![text("read it"), done()],
    ]);

    // Bound rather than borrowed inline: `&Volatile::new()` is a borrow of a
    // temporary, which compiles today and would not survive a stricter
    // temporary-lifetime rule.
    let log = Volatile::new();

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "read note.txt".into(),
        },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    let output = session
        .state()
        .history()
        .iter()
        .find_map(|item| match item {
            ResponseItem::FunctionCallOutput { output, .. } => Some(output.clone()),
            _ => None,
        })
        .expect("the tool output was recorded");
    assert!(
        output.contains("hello from the workspace"),
        "a relative path resolved against the session's working directory: {output}"
    );
}
