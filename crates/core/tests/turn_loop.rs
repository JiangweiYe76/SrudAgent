//! End-to-end tests for the turn loop against a scripted model.

use std::sync::{Arc, Mutex};

use futures::stream;
use srud_core::client::{
    ModelClient, ModelError, ModelEvent, ModelRequest, ModelRequestItem, ModelStream,
};
use srud_core::session::Session;
use srud_core::session_log::Volatile;
use srud_core::tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolRegistry};
use srud_core::types::{
    Event, EventSink, ImagePart, Op, ResponseItem, Role, TurnEndReason, TurnInput,
};
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
        Event::ToolCallNamed { .. } => "ToolCallNamed",
        Event::UsageRecorded { .. } => "UsageRecorded",
        Event::TurnDiscarded { .. } => "TurnDiscarded",
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
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
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
async fn a_reported_usage_is_recorded_and_announced() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    let usage = srud_core::client::TokenUsage {
        input_tokens: 120,
        output_tokens: 30,
        cached_input_tokens: 100,
        cache_write_input_tokens: 20,
        reasoning_output_tokens: 8,
    };
    let client = Scripted::new(vec![vec![text("Hi"), Ok(ModelEvent::Usage(usage)), done()]]);

    let log = Volatile::new();

    let result = srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    assert_eq!(result.reason, TurnEndReason::Completed);

    // The announcement follows the message its request produced: the usage is
    // the cost of that request, and a consumer pairing them reads them in order.
    let kinds = sink.kinds();
    let usage_at = kinds
        .iter()
        .position(|k| *k == "UsageRecorded")
        .expect("usage is announced");
    let message_at = kinds
        .iter()
        .rposition(|k| *k == "AgentMessageDelta")
        .expect("message is announced");
    assert!(
        usage_at > message_at,
        "usage comes after the message it costs: {kinds:?}"
    );

    let recorded: Vec<_> = log.records().into_iter().map(|line| line.event).collect();
    assert!(
        recorded.contains(&srud_core::SessionEvent::UsageRecorded {
            turn_id: result.turn_id,
            usage,
        }),
        "the usage record is durable: {recorded:?}"
    );
}

#[tokio::test]
async fn a_provider_that_reports_nothing_produces_no_usage_record() {
    let session = session();
    let sink = Recorder::default();
    let (tools, _calls) = tool_registry();
    let client = Scripted::new(vec![vec![text("Hi"), done()]]);
    let log = Volatile::new();

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    assert!(!sink.kinds().contains(&"UsageRecorded"));
    assert!(
        !log.records()
            .iter()
            .any(|line| matches!(line.event, srud_core::SessionEvent::UsageRecorded { .. })),
        "absence is not recorded as zeroes"
    );
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
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
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
        TurnInput {
            text: "go".into(),
            images: Vec::new(),
        },
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
            srud_core::client::ModelRequestItem::Image { .. } => "image",
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
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
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
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
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
        TurnInput {
            text: "go".into(),
            images: Vec::new(),
        },
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
        TurnInput {
            text: "go".into(),
            images: Vec::new(),
        },
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
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
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
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
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
        Op::TurnInput(TurnInput {
            text: "x".into(),
            images: Vec::new(),
        }),
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
        TurnInput {
            text: "go".into(),
            images: Vec::new(),
        },
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
            images: Vec::new(),
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

#[tokio::test]
async fn a_turn_that_opens_on_an_answered_message_keeps_what_the_last_one_wrote_before_it() {
    // Regenerating: the question stays in history, the answer it produced is
    // cut, and the request that follows carries the question once.
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();
    let first_sink = Recorder::default();
    let first = Scripted::new(vec![vec![text("a first answer"), done()]]);

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        first.as_ref(),
        &tools,
        &log,
        &first_sink,
    )
    .await
    .expect("turn runs");

    // Taken before the turn opened, the way a caller has to: the mark is how
    // far the history had got when the question was the last thing in it.
    let opening = Some(2);
    assert!(
        session.truncate(2),
        "the answer is what answers the question"
    );

    let second_sink = Recorder::default();
    let second = Scripted::new(vec![vec![text("a second answer"), done()]]);

    let result = srud_core::run_turn_after(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        opening,
        second.as_ref(),
        &tools,
        &log,
        &second_sink,
    )
    .await
    .expect("turn runs");

    assert_eq!(result.reason, TurnEndReason::Completed);
    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("hi"),
            ResponseItem::assistant("a second answer"),
        ][..],
        "the question survived once, and only the second answer is there"
    );
    assert_eq!(
        second_sink.kinds(),
        vec!["TurnStarted", "AgentMessageDelta", "TurnComplete"],
        "nothing is said again: the user's message is already in the conversation"
    );

    let requests = second.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .items
            .iter()
            .filter(|item| matches!(item, ModelRequestItem::Message { role: Role::User, content } if content == "hi"))
            .count(),
        1,
        "the model is asked once about the question: {:?}",
        requests[0].items
    );
    assert!(
        !requests[0]
            .items
            .iter()
            .any(|item| matches!(item, ModelRequestItem::Message { role: Role::Assistant, content } if content == "a first answer")),
        "and never sees the answer that was discarded"
    );
}

#[tokio::test]
async fn the_mark_a_re_answer_cuts_at_keeps_the_question() {
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        Scripted::new(vec![vec![text("an answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("turn runs");

    let opened = session
        .last_prompt()
        .expect("the last turn can be asked again");
    // The context block and the question: everything the answer was built on,
    // and nothing it wrote.
    assert_eq!(
        opened.mark, 2,
        "the mark sits after the question, not before it"
    );
    assert!(session.truncate(opened.mark), "the answer is cut back");
    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("hi")
        ][..],
        "the question survives the cut, which is what lets the re-answer repeat it"
    );
}

#[tokio::test]
async fn a_re_answer_from_the_recorded_mark_asks_the_question_once() {
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        Scripted::new(vec![vec![text("a first answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("turn runs");

    let opened = session
        .last_prompt()
        .expect("the last turn can be asked again");
    assert!(session.truncate(opened.mark), "the answer is cut back");

    let sink = Recorder::default();
    let second = Scripted::new(vec![vec![text("a second answer"), done()]]);
    srud_core::run_turn_after(
        &session,
        opened.input,
        Some(opened.mark),
        second.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("re-answer runs");

    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("hi"),
            ResponseItem::assistant("a second answer"),
        ][..],
        "the question is in the history once, with only the second answer under it"
    );
    assert_eq!(
        sink.kinds(),
        vec!["TurnStarted", "AgentMessageDelta", "TurnComplete"],
        "nothing is said again: the question was already there"
    );

    let requests = second.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]
            .items
            .iter()
            .filter(|item| matches!(item, ModelRequestItem::Message { role: Role::User, content } if content == "hi"))
            .count(),
        1,
        "the question is put to the model once"
    );
    assert!(
        !requests[0].items.iter().any(|item| matches!(
            item,
            ModelRequestItem::Message { role: Role::Assistant, content } if content == "a first answer"
        )),
        "and the answer that was discarded is not sent again: {:?}",
        requests[0].items
    );
}

#[tokio::test]
async fn a_restored_session_keeps_the_last_question() {
    // What a log rebuild hands back after a restart: no turn loop ran in this
    // process, so the mark has to come from the history itself.
    let session = Session::restored(
        srud_core::types::SessionId::new(),
        "workspace",
        vec![
            srud_core::context::item(std::path::Path::new("workspace")),
            ResponseItem::user("hi"),
            ResponseItem::assistant("an answer"),
        ],
    );

    let opened = session
        .last_prompt()
        .expect("the last question can still be asked again");
    assert_eq!(opened.mark, 2, "the mark sits after the question");
    assert_eq!(opened.input.text, "hi");

    assert!(session.truncate(opened.mark));
    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(std::path::Path::new("workspace")),
            ResponseItem::user("hi"),
        ][..],
        "cutting the answer leaves the question"
    );
}

#[tokio::test]
async fn a_re_answer_keeps_the_images_it_was_sent_with() {
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();
    let shot = ImagePart {
        mime: "image/png".into(),
        data: "YWJj".into(),
    };

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "look".into(),
            images: vec![shot.clone()],
        },
        Scripted::new(vec![vec![text("a first answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("turn runs");

    let opened = session.last_prompt().expect("can be asked again");
    assert_eq!(
        opened.mark, 3,
        "the mark sits past the image as well as the text"
    );
    assert_eq!(opened.input.images.len(), 1, "the image comes back with it");

    assert!(session.truncate(opened.mark));
    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("look"),
            ResponseItem::Image {
                mime: "image/png".into(),
                data: "YWJj".into(),
            },
        ][..],
        "cutting the answer leaves the text and the image"
    );

    srud_core::run_turn_after(
        &session,
        opened.input,
        Some(opened.mark),
        Scripted::new(vec![vec![text("a second answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("re-answer runs");

    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("look"),
            ResponseItem::Image {
                mime: "image/png".into(),
                data: "YWJj".into(),
            },
            ResponseItem::assistant("a second answer"),
        ][..],
        "the image is in the history once, not twice"
    );
}

#[tokio::test]
async fn a_restored_session_keeps_the_images_beside_the_question() {
    let session = Session::restored(
        srud_core::types::SessionId::new(),
        "workspace",
        vec![
            srud_core::context::item(std::path::Path::new("workspace")),
            ResponseItem::user("look"),
            ResponseItem::Image {
                mime: "image/png".into(),
                data: "YWJj".into(),
            },
            ResponseItem::assistant("an answer"),
        ],
    );

    let opened = session.last_prompt().expect("can be asked again");
    assert_eq!(opened.mark, 3, "the mark reaches past the image");
    assert_eq!(opened.input.text, "look");
    assert_eq!(opened.input.images.len(), 1);
    assert_eq!(opened.input.images[0].mime, "image/png");
}

#[tokio::test]
async fn a_restored_session_with_no_user_message_cannot_be_asked_again() {
    let session = Session::restored(
        srud_core::types::SessionId::new(),
        "workspace",
        vec![srud_core::context::item(std::path::Path::new("workspace"))],
    );
    assert!(session.last_prompt().is_none());
}

#[tokio::test]
async fn a_second_re_answer_cuts_at_the_same_question() {
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        Scripted::new(vec![vec![text("a first answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("turn runs");

    let opened = session.last_prompt().expect("can be asked again");
    assert!(session.truncate(opened.mark));
    srud_core::run_turn_after(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        Some(opened.mark),
        Scripted::new(vec![vec![text("a second answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("re-answer runs");

    // The replacement records no input of its own, so the mark still points at
    // the question: a second re-answer cuts the same place, not one answer later.
    let again = session.last_prompt().expect("can be asked again");
    assert_eq!(again.mark, opened.mark, "the mark did not move");
    assert!(session.truncate(again.mark));
    srud_core::run_turn_after(
        &session,
        again.input,
        Some(again.mark),
        Scripted::new(vec![vec![text("a third answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("second re-answer runs");

    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("hi"),
            ResponseItem::assistant("a third answer"),
        ][..],
        "the question is still there once, with only the newest answer"
    );
}

#[tokio::test]
async fn a_re_answer_stopped_mid_answer_keeps_the_question() {
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        Scripted::new(vec![vec![text("a first answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("turn runs");

    let opened = session.last_prompt().expect("can be asked again");
    assert!(session.truncate(opened.mark));

    // A model that only answers once it is cancelled, so the stop lands while
    // the replacement is still running rather than after it.
    let stalled = Arc::new(Stalled {
        started: tokio::sync::Notify::new(),
    });
    let stop = async {
        stalled.started.notified().await;
        session.interrupt();
    };
    let sink = Recorder::default();
    let (result, ()) = tokio::join!(
        srud_core::run_turn_after(
            &session,
            TurnInput {
                text: "hi".into(),
                images: Vec::new(),
            },
            Some(opened.mark),
            stalled.as_ref(),
            &tools,
            &log,
            &sink,
        ),
        stop
    );

    assert_eq!(
        result.expect("the turn ends").reason,
        TurnEndReason::Interrupted
    );
    assert_eq!(
        session.state().history(),
        &[
            srud_core::context::item(session.cwd()),
            ResponseItem::user("hi"),
            // Stopping ends the turn; it does not take back what the model had
            // already said before it noticed.
            ResponseItem::assistant("late"),
        ][..],
        "the question stays once, and the answer that was cut is not repeated"
    );
    assert_eq!(
        sink.kinds(),
        vec!["TurnStarted", "AgentMessageDelta", "TurnComplete"],
        "the late delta is the wake-up, and the turn still closes once"
    );
}

/// Answers nothing until its turn is cancelled.
struct Stalled {
    started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl ModelClient for Stalled {
    async fn stream(
        &self,
        _request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        self.started.notify_one();
        let tail = stream::unfold(Some(cancel), |state| async move {
            let token = state?;
            token.cancelled().await;
            Some((
                Ok(ModelEvent::TextDelta {
                    delta: "late".into(),
                }),
                None,
            ))
        });
        Ok(Box::pin(tail))
    }
}

#[tokio::test]
async fn a_discarded_turn_is_recorded_and_announced_before_the_new_one() {
    let session = session();
    let log = Volatile::new();
    let (tools, _calls) = tool_registry();
    let sink = Recorder::default();
    let client = Scripted::new(vec![vec![text("an answer"), done()]]);

    let earlier = srud_core::run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        Scripted::new(vec![vec![text("the answer"), done()]]).as_ref(),
        &tools,
        &log,
        &Recorder::default(),
    )
    .await
    .expect("turn runs");

    let journal = srud_core::turn::Journal::new(&session, &log, &sink, earlier.turn_id);
    journal.discard().await.expect("the discard is recorded");
    assert_eq!(sink.kinds(), vec!["TurnDiscarded"]);

    srud_core::run_turn(
        &session,
        TurnInput {
            text: "again".into(),
            images: Vec::new(),
        },
        client.as_ref(),
        &tools,
        &log,
        &sink,
    )
    .await
    .expect("turn runs");

    let records = log.records();
    let kinds: Vec<&str> = records
        .iter()
        .map(|logged| match &logged.event {
            srud_core::SessionEvent::TurnDiscarded { .. } => "turn_discarded",
            srud_core::SessionEvent::TurnStarted { .. } => "turn_started",
            _ => "other",
        })
        .collect();
    let discard = kinds
        .iter()
        .position(|kind| *kind == "turn_discarded")
        .expect("the discard was written");
    assert_eq!(
        kinds[discard + 1],
        "turn_started",
        "the discard is written before anything the new turn records: {kinds:?}"
    );
}
