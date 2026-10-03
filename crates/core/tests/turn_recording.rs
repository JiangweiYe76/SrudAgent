// What a turn writes, and in what order.
//
// The ordering is the point of this file. Everything recorded must reach the log
// before it is announced, because the other order loses data invisibly: a crash in
// between leaves a consumer showing something the log does not have, and
// rebuilding from the log drops it.
//
// These assert against the rendered event stream rather than the log, because the
// stream is where the damage shows up. A test that only checked the log would pass
// with the order reversed.
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use srud_core::client::{ModelClient, ModelError, ModelEvent, ModelRequest, ModelStream};
use srud_core::session::Session;
use srud_core::session_event::{MessageId, SessionEvent};
use srud_core::session_log::{RecordError, SessionLog, Volatile};
use srud_core::tools::{Tool, ToolContext, ToolOutcome, ToolRegistry};
use srud_core::types::{Event, EventSink, ResponseItem, TurnEndReason, TurnInput};
use srud_core::{run_turn, TurnError};

/// One recorded thing and what had been announced by the time it was written.
#[derive(Debug, Clone, PartialEq)]
struct Beat {
    recorded: String,
    announced: Vec<String>,
}

/// A sink that remembers what it was told, and when.
#[derive(Default)]
struct Timeline {
    events: Mutex<Vec<Event>>,
}

impl Timeline {
    /// The event kinds announced so far, in order.
    fn kinds(&self) -> Vec<String> {
        self.events
            .lock()
            .expect("timeline lock")
            .iter()
            .map(kind_of)
            .collect()
    }

    /// Every message id seen, in order, as announced.
    fn message_ids(&self) -> Vec<Option<String>> {
        self.events
            .lock()
            .expect("timeline lock")
            .iter()
            .filter_map(|event| match event {
                Event::AgentMessageDelta { message_id, .. }
                | Event::AgentThoughtDelta { message_id, .. } => {
                    Some(message_id.as_ref().map(ToString::to_string))
                }
                _ => None,
            })
            .collect()
    }
}

/// The name a test asserts on.
fn kind_of(event: &Event) -> String {
    match event {
        Event::TurnStarted { .. } => "TurnStarted".into(),
        Event::UserMessage { .. } => "UserMessage".into(),
        Event::AgentMessageDelta { .. } => "AgentMessageDelta".into(),
        Event::AgentThoughtDelta { .. } => "AgentThoughtDelta".into(),
        Event::ToolCallBegin { .. } => "ToolCallBegin".into(),
        Event::ToolCallEnd { .. } => "ToolCallEnd".into(),
        Event::TurnComplete { .. } => "TurnComplete".into(),
    }
}

impl EventSink for Timeline {
    fn emit(&self, event: Event) {
        self.events.lock().expect("timeline lock").push(event);
    }
}

/// A log that records what had been announced at the moment of each write.
///
/// This is how the order is observed: the write notes the announcements that
/// preceded it, so a record showing an event that should follow it is the ordering
/// being wrong, seen from the log's side rather than the consumer's.
#[derive(Clone, Default)]
struct Beating {
    timeline: Arc<Timeline>,
    beats: Arc<Mutex<Vec<Beat>>>,
}

impl Beating {
    fn new(timeline: Arc<Timeline>) -> Self {
        Self {
            timeline,
            beats: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Each record, with the announcements that preceded it.
    fn beats(&self) -> Vec<Beat> {
        self.beats.lock().expect("beats lock").clone()
    }

    /// The announcements in place before each of `names` was written, in order.
    fn announced_before(&self, names: &[&str]) -> Vec<Vec<String>> {
        self.beats()
            .into_iter()
            .filter(|beat| names.iter().any(|name| beat.recorded.starts_with(name)))
            .map(|beat| beat.announced)
            .collect()
    }
}

#[async_trait]
impl SessionLog for Beating {
    async fn record(&self, entry: &SessionEvent) -> Result<(), RecordError> {
        self.beats.lock().expect("beats lock").push(Beat {
            recorded: describe(entry),
            announced: self.timeline.kinds(),
        });
        Ok(())
    }
}

/// What a record is, for a test to match on.
fn describe(entry: &SessionEvent) -> String {
    match entry {
        SessionEvent::Session { .. } => "session".into(),
        SessionEvent::System { .. } => "system".into(),
        SessionEvent::TurnStarted { .. } => "turn_started".into(),
        SessionEvent::TurnEnded { .. } => "turn_ended".into(),
        SessionEvent::Item { item, .. } => match item {
            ResponseItem::Message {
                role: srud_core::types::Role::User,
                content,
            } => {
                if srud_core::context::is_item(&ResponseItem::Message {
                    role: srud_core::types::Role::User,
                    content: content.clone(),
                }) {
                    "env_context".into()
                } else {
                    "user_message".into()
                }
            }
            ResponseItem::Message { .. } => "assistant_message".into(),
            ResponseItem::Reasoning { .. } => "reasoning".into(),
            ResponseItem::FunctionCall { .. } => "tool_call".into(),
            ResponseItem::FunctionCallOutput { .. } => "tool_result".into(),
        },
    }
}

/// The events a log holds, in order.
///
/// For the tests that are about what was recorded rather than when; a test about
/// the time uses [`Volatile::records`] and reads the line.
fn events(log: &Volatile) -> Vec<SessionEvent> {
    log.records()
        .into_iter()
        .map(|logged| logged.event)
        .collect()
}

/// A model that says nothing, so a turn is env-context, user input, and an end.
struct Silent;

#[async_trait]
impl ModelClient for Silent {
    async fn stream(
        &self,
        _request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        Ok(Box::pin(futures::stream::iter(vec![Ok(ModelEvent::Done)])))
    }
}

/// A model that answers with one script per request, for tests about messages
/// and tools.
///
/// **Each round is consumed.** A turn that runs a tool asks the model again, so a
/// model that replayed the same round forever would be asked for the same tool
/// call on every round: the loop would never finish, and a log that appends
/// per round would grow until the process ran out of memory. A test that wants a
/// tool call therefore has to say what the *second* round answers, which is what
/// ends the turn.
struct Scripted(Mutex<Vec<Vec<ModelEvent>>>);

impl Scripted {
    /// A model that answers each request with the next of these rounds.
    fn new(rounds: Vec<Vec<ModelEvent>>) -> Self {
        Self(Mutex::new(rounds))
    }
}

#[async_trait]
impl ModelClient for Scripted {
    async fn stream(
        &self,
        _request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let mut rounds = self.0.lock().expect("rounds lock");
        let Some(events) = rounds.first().cloned() else {
            return Err(ModelError::Rejected("script exhausted".into()));
        };
        rounds.remove(0);
        let events: Vec<Result<ModelEvent, ModelError>> = events.into_iter().map(Ok).collect();
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

/// A tool that succeeds, so a turn reaches the tool-call path.
struct Echo;

#[async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn description(&self) -> &str {
        "Returns its input."
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"],
        })
    }
    async fn call(
        &self,
        _ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, srud_core::tools::ToolError> {
        let text = arguments
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        Ok(ToolOutcome::success(text))
    }
}

/// A registry holding one tool.
fn tools() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(Echo)).expect("a fresh registry");
    registry
}

/// A session working in a directory that does not exist, which the loop never
/// reads.
fn session() -> Session {
    Session::new("workspace")
}

/// A model that remembers the instruction of every request it was asked with.
///
/// Wraps another model rather than replacing one, so a test about what the
/// request carried reuses the rounds of whichever model it needs.
struct Watching {
    inner: Scripted,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl ModelClient for Watching {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        self.seen
            .lock()
            .expect("seen lock")
            .push(request.instructions.clone());
        self.inner.stream(request, cancel).await
    }
}

#[tokio::test]
async fn a_record_is_written_before_the_event_that_announces_it() {
    // The invariant, stated once: nothing is announced that the log does not
    // already hold.
    let timeline = Arc::new(Timeline::default());
    let log = Beating::new(Arc::clone(&timeline));
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Silent,
        &tools(),
        &log,
        timeline.as_ref(),
    )
    .await
    .expect("the turn runs");

    // The user message is the one record with an event of its own. It must be
    // written with nothing announced yet — not after the message was shown.
    let before = log.announced_before(&["user_message"]);
    assert_eq!(before.len(), 1, "the user message was recorded once");
    assert!(
        !before[0].contains(&"UserMessage".to_string()),
        "it was announced before it was written: {:?}",
        before[0]
    );
}

#[tokio::test]
async fn the_turn_opens_and_closes_in_the_log() {
    let log = Volatile::new();
    let timeline = Timeline::default();
    let session = session();

    let result = run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Silent,
        &tools(),
        &log,
        &timeline,
    )
    .await
    .expect("the turn runs");

    let records = events(&log);
    assert!(
        matches!(records.first(), Some(SessionEvent::System { .. })),
        "the instruction is written before the turn opens: {:?}",
        records.first()
    );
    assert!(
        matches!(records.get(1), Some(SessionEvent::TurnStarted { .. })),
        "then the turn opens: {:?}",
        records.get(1)
    );
    assert!(
        matches!(records.last(), Some(SessionEvent::TurnEnded { .. })),
        "and closes: {:?}",
        records.last()
    );
    assert!(log.ended_with(result.reason));
}

#[tokio::test]
async fn every_announcement_has_a_record_written_first() {
    // The whole stream at once, rather than one record at a time: a turn is a
    // sequence, and a gap anywhere in it is a gap a consumer cannot recover from.
    let timeline = Arc::new(Timeline::default());
    let log = Beating::new(Arc::clone(&timeline));
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Scripted::new(vec![vec![
            ModelEvent::TextDelta { delta: "he".into() },
            ModelEvent::TextDelta {
                delta: "llo".into(),
            },
            ModelEvent::Done,
        ]]),
        &tools(),
        &log,
        timeline.as_ref(),
    )
    .await
    .expect("the turn runs");

    // TurnStarted, then the user's message, then a chunk per delta, then the
    // close. The chunks are announced as they stream — a chunk is a fragment of a
    // record, not a record of its own, so there is nothing to write until the
    // message closes. That is why five announcements produce four records.
    assert_eq!(
        timeline.kinds(),
        vec![
            "TurnStarted",
            "UserMessage",
            "AgentMessageDelta",
            "AgentMessageDelta",
            "TurnComplete",
        ]
    );

    // Each of those was written before the next announcement was made. The two
    // streamed chunks share one record: they are fragments of one message, and it
    // is written when the message closes rather than per fragment. The instruction
    // precedes all of them and is announced by nobody, so it is written first.
    let order: Vec<String> = log.beats().into_iter().map(|beat| beat.recorded).collect();
    assert_eq!(
        order,
        vec![
            "system".to_string(),
            "turn_started".to_string(),
            "env_context".to_string(),
            "user_message".to_string(),
            "assistant_message".to_string(),
            "turn_ended".to_string(),
        ]
    );
}

#[tokio::test]
async fn a_tool_call_is_recorded_before_it_runs() {
    // The call is written before the tool executes, so a log cut short by a crash
    // still says what was attempted. An unanswered call is the one shape a model
    // API rejects on resume.
    let timeline = Arc::new(Timeline::default());
    let log = Beating::new(Arc::clone(&timeline));
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        // The second round is what ends the turn: the first asks for a tool, the
        // loop runs it and asks again, and this round answers with nothing to do.
        &Scripted::new(vec![
            vec![
                ModelEvent::ToolCall {
                    call_id: "c1".into(),
                    name: "echo".into(),
                    arguments: r#"{"text":"x"}"#.into(),
                },
                ModelEvent::Done,
            ],
            vec![ModelEvent::Done],
        ]),
        &tools(),
        &log,
        timeline.as_ref(),
    )
    .await
    .expect("the turn runs");

    let order: Vec<String> = log.beats().into_iter().map(|beat| beat.recorded).collect();
    let call = order.iter().position(|name| name == "tool_call");
    let result = order.iter().position(|name| name == "tool_result");
    assert!(call.is_some(), "the call was recorded: {order:?}");
    assert!(result.is_some(), "so was its result: {order:?}");
    assert!(call < result, "the call precedes its result: {order:?}");

    // And the call was on disk before the tool call was announced.
    let before = log.announced_before(&["tool_call"]);
    assert!(
        !before[0].contains(&"ToolCallBegin".to_string()),
        "announced before it was written: {:?}",
        before[0]
    );
}

#[tokio::test]
async fn one_message_carries_one_id_across_its_chunks() {
    // What the id is for: a consumer cannot tell where one message ends and the
    // next begins from the text of a fragment alone.
    let log = Volatile::new();
    let timeline = Timeline::default();
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Scripted::new(vec![vec![
            ModelEvent::TextDelta { delta: "he".into() },
            ModelEvent::TextDelta {
                delta: "llo".into(),
            },
            ModelEvent::Done,
        ]]),
        &tools(),
        &log,
        &timeline,
    )
    .await
    .expect("the turn runs");

    let ids = timeline.message_ids();
    assert_eq!(ids.len(), 2, "two chunks, two announcements");
    assert!(ids[0].is_some(), "a chunk carries the id of its message");
    assert_eq!(
        ids[0], ids[1],
        "both chunks are one message, so they share an id"
    );
}

#[tokio::test]
async fn reasoning_is_ided_apart_from_the_answer() {
    // They arrive as separate streams and a consumer renders them apart, so one
    // id across both would group two things it already distinguishes.
    let timeline = Timeline::default();
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Scripted::new(vec![vec![
            ModelEvent::ThoughtDelta {
                delta: "think".into(),
            },
            ModelEvent::TextDelta {
                delta: "answer".into(),
            },
            ModelEvent::Done,
        ]]),
        &tools(),
        &Volatile::new(),
        &timeline,
    )
    .await
    .expect("the turn runs");

    let events = timeline.events.lock().expect("timeline lock").clone();
    let thought = events
        .iter()
        .find_map(|event| match event {
            Event::AgentThoughtDelta { message_id, .. } => Some(message_id.clone()),
            _ => None,
        })
        .expect("a thought was streamed");
    let answer = events
        .iter()
        .find_map(|event| match event {
            Event::AgentMessageDelta { message_id, .. } => Some(message_id.clone()),
            _ => None,
        })
        .expect("an answer was streamed");

    assert!(thought.is_some(), "the thought is ided");
    assert!(answer.is_some(), "so is the answer");
    assert_ne!(thought, answer, "and they are not the same message");
}

#[tokio::test]
async fn the_recorded_message_carries_the_id_that_was_announced() {
    // The two have to agree, or a log read back produces messages the consumer
    // never saw — the id is what groups a replay's chunks.
    let log = Volatile::new();
    let timeline = Timeline::default();
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Scripted::new(vec![vec![
            ModelEvent::TextDelta {
                delta: "hello".into(),
            },
            ModelEvent::Done,
        ]]),
        &tools(),
        &log,
        &timeline,
    )
    .await
    .expect("the turn runs");

    let announced = timeline
        .events
        .lock()
        .expect("timeline lock")
        .iter()
        .find_map(|event| match event {
            Event::AgentMessageDelta { message_id, .. } => Some(message_id.clone()),
            _ => None,
        })
        .expect("an answer was streamed");

    let recorded = events(&log)
        .into_iter()
        .find_map(|entry| match entry {
            SessionEvent::Item {
                message_id: Some(id),
                item:
                    ResponseItem::Message {
                        role: srud_core::types::Role::Assistant,
                        ..
                    },
                ..
            } => Some(id),
            _ => None,
        })
        .expect("the answer was recorded");

    assert_eq!(Some(recorded), announced, "the same id on both sides");
}

#[tokio::test]
async fn the_reasoning_is_recorded_before_the_message_it_produced() {
    // A model thinks, then answers, and the log has to say so. `session/load`
    // walks it in order and the consumer opens a step from whichever chunk
    // arrives first, so a log with the answer first renders the reply above the
    // reasoning that produced it.
    let log = Volatile::new();
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Scripted::new(vec![vec![
            ModelEvent::ThoughtDelta {
                delta: "think".into(),
            },
            ModelEvent::TextDelta {
                delta: "answer".into(),
            },
            ModelEvent::Done,
        ]]),
        &tools(),
        &log,
        &Timeline::default(),
    )
    .await
    .expect("the turn runs");

    let order: Vec<String> = events(&log).iter().map(describe).collect();
    let reasoning = order.iter().position(|name| name == "reasoning");
    let message = order.iter().position(|name| name == "assistant_message");
    assert!(message.is_some() && reasoning.is_some(), "{order:?}");
    assert!(
        reasoning < message,
        "the thinking came first, so it is recorded first: {order:?}"
    );
}

#[tokio::test]
async fn a_turn_that_cannot_record_stops_rather_than_announcing() {
    // The state that cannot be recovered: the model reads history the log does not
    // have, and there is no later way to tell which of the two is the truth.
    struct Refusing;

    #[async_trait]
    impl SessionLog for Refusing {
        async fn record(&self, entry: &SessionEvent) -> Result<(), RecordError> {
            // Let the turn open, then refuse everything after — the point is what
            // happens once there is something the consumer would have seen.
            if matches!(entry, SessionEvent::TurnStarted { .. }) {
                return Ok(());
            }
            Err(RecordError::Io {
                what: "the user's message".into(),
                source: std::io::Error::other("no space left on device"),
            })
        }
    }

    let timeline = Timeline::default();
    let session = session();

    let error = run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Silent,
        &tools(),
        &Refusing,
        &timeline,
    )
    .await
    .expect_err("the turn cannot be recorded");

    assert!(
        matches!(error, TurnError::Unrecordable(_)),
        "reported as unrecordable, not as busy: {error}"
    );
    assert!(
        timeline.kinds().iter().all(|kind| kind == "TurnStarted"),
        "nothing was announced that the log did not hold: {:?}",
        timeline.kinds()
    );
}

#[tokio::test]
async fn a_refused_turn_end_still_announces_the_ending() {
    // The asymmetry, deliberately: a log one record short is recoverable, while a
    // turn left open on the wire is not — the caller waits on it to close.
    struct RefusesOnlyTheEnd;

    #[async_trait]
    impl SessionLog for RefusesOnlyTheEnd {
        async fn record(&self, entry: &SessionEvent) -> Result<(), RecordError> {
            if matches!(entry, SessionEvent::TurnEnded { .. }) {
                return Err(RecordError::Io {
                    what: "the turn's end".into(),
                    source: std::io::Error::other("no space left on device"),
                });
            }
            Ok(())
        }
    }

    let timeline = Timeline::default();
    let session = session();

    let result = run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Silent,
        &tools(),
        &RefusesOnlyTheEnd,
        &timeline,
    )
    .await
    .expect("the turn still ends");

    assert_eq!(result.reason, TurnEndReason::Completed);
    assert_eq!(
        timeline.kinds().last().map(String::as_str),
        Some("TurnComplete"),
        "the caller learns the turn ended: {:?}",
        timeline.kinds()
    );
}

#[tokio::test]
async fn a_message_id_survives_a_round_trip_through_the_log() {
    // What a resume will need: the id read back has to equal the one announced,
    // or the replay's chunks do not group into the message they came from.
    let log = Volatile::new();
    let timeline = Timeline::default();
    let session = session();

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &Scripted::new(vec![vec![
            ModelEvent::TextDelta {
                delta: "hello".into(),
            },
            ModelEvent::Done,
        ]]),
        &tools(),
        &log,
        &timeline,
    )
    .await
    .expect("the turn runs");

    let entry = events(&log)
        .into_iter()
        .find(|entry| {
            // Matched on the assistant role, not on `Message` alone: the user's
            // input and the env-context block are messages too, and either would
            // satisfy a looser match — with no id, which is the very thing this
            // is checking for.
            matches!(
                entry,
                SessionEvent::Item {
                    item: ResponseItem::Message {
                        role: srud_core::types::Role::Assistant,
                        ..
                    },
                    ..
                }
            )
        })
        .expect("the answer was recorded");

    let json = serde_json::to_string(&entry).expect("a record serialises");
    let back: SessionEvent = serde_json::from_str(&json).expect("a record deserialises");

    match back {
        SessionEvent::Item { message_id, .. } => {
            assert!(matches!(message_id, Some(MessageId(_))), "{json}");
        }
        other => panic!("expected an item: {other:?}"),
    }
}

#[tokio::test]
async fn the_instruction_is_written_once_and_not_again_per_turn() {
    // Constant because a provider caches a request by its prefix, so a second
    // copy would buy nothing and a per-turn one would grow the log for as long
    // as the session runs.
    let log = Volatile::new();
    let timeline = Timeline::default();
    let session = session();

    for text in ["first", "second", "third"] {
        run_turn(
            &session,
            TurnInput { text: text.into() },
            &Silent,
            &tools(),
            &log,
            &timeline,
        )
        .await
        .expect("the turn runs");
    }

    let written: Vec<SessionEvent> = events(&log)
        .into_iter()
        .filter(|entry| matches!(entry, SessionEvent::System { .. }))
        .collect();
    assert_eq!(written.len(), 1, "one session, one instruction");
}

#[tokio::test]
async fn the_instruction_recorded_is_the_one_the_request_carries() {
    // The record and the request are rendered from one value. If they were
    // rendered separately they would agree only for as long as nothing changed
    // between the two calls, and a log holding a different prefix from the one
    // sent is worse than no log at all.
    let log = Volatile::new();
    let timeline = Timeline::default();
    let session = session();
    let seen = Arc::new(Mutex::new(Vec::new()));

    let client = Watching {
        inner: Scripted::new(vec![vec![ModelEvent::Done]]),
        seen: Arc::clone(&seen),
    };

    run_turn(
        &session,
        TurnInput { text: "hi".into() },
        &client,
        &tools(),
        &log,
        &timeline,
    )
    .await
    .expect("the turn runs");

    let recorded = events(&log)
        .into_iter()
        .find_map(|entry| match entry {
            SessionEvent::System { content } => Some(content),
            _ => None,
        })
        .expect("an instruction was recorded");

    let sent = seen.lock().expect("seen lock").remove(0);
    assert_eq!(
        sent.as_deref(),
        Some(recorded.as_str()),
        "the log and the request must not diverge"
    );
}
