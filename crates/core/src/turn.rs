//! The turn loop.
//!
//! [`run_turn`] is the only writer of session history and the only emitter of
//! [`Event`]s. Every exit path funnels through a single `TurnComplete`, so a
//! turn always ends exactly once, and no other code competes to emit.
//!
//! # Recording
//!
//! Every recorded thing goes through [`Journal::record`], which is what makes the
//! ordering rule structural rather than remembered: the record is written and
//! flushed, then history is updated, then the event goes out. A call site cannot
//! emit first because it has no way to — the emit happens inside the one function
//! that has already written.
//!
//! The rule matters because the other order loses data silently. Emitting first
//! and crashing before the write leaves a consumer showing something the log does
//! not have, and rebuilding from the log drops it.
//!
//! A turn that cannot record does not announce anything it has not recorded: it
//! ends with [`TurnEndReason::Error`] instead, which costs the user the messages
//! of one turn rather than the guarantee that what they saw can be recovered.

use futures::StreamExt;

use crate::client::{ModelClient, ModelEvent, ModelRequest};
use crate::context;
use crate::prompt;
use crate::session::Session;
use crate::session_event::MessageId;
use crate::session_log::{entry_for, RecordError, SessionLog};
use crate::tools::{failure_text, ToolContext, ToolError, ToolOutcome, ToolRegistry};
use crate::types::{Event, EventSink, ResponseItem, Role, TurnEndReason, TurnId, TurnInput};

/// What a finished turn produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnResult {
    /// Which turn this was.
    pub turn_id: TurnId,
    /// Why it stopped.
    pub reason: TurnEndReason,
}

/// One turn's recording, in the only order these three things may happen.
///
/// Held for the length of a turn so that the write and the announcement cannot be
/// reached independently: [`Journal::record`] is the one path from a history entry
/// to an event, so a caller cannot announce something it has not recorded.
///
/// Not every record is announced. The env-context block is context the model
/// reads rather than something the user said, and a turn's end is announced by
/// the caller that closes it. Those pass no event, and that is the whole meaning
/// of the parameter being optional.
pub struct Journal<'a> {
    session: &'a Session,
    log: &'a dyn SessionLog,
    sink: &'a dyn EventSink,
    turn_id: TurnId,
}

impl<'a> Journal<'a> {
    /// Starts a turn's recording.
    pub fn new(
        session: &'a Session,
        log: &'a dyn SessionLog,
        sink: &'a dyn EventSink,
        turn_id: TurnId,
    ) -> Self {
        Self {
            session,
            log,
            sink,
            turn_id,
        }
    }

    /// Records a history entry, then announces whatever the entry produced.
    ///
    /// The event is optional because not every entry produces one: the
    /// env-context block is context the model reads rather than something the
    /// user said, and there is nothing to show for it.
    ///
    /// # Errors
    ///
    /// Fails if the record could not be written. The history is **not** updated
    /// and the event is **not** emitted in that case: a turn that has announced
    /// something the log does not hold cannot be recovered from the log, which is
    /// the whole reason the write comes first.
    pub async fn record(
        &self,
        message_id: Option<MessageId>,
        item: ResponseItem,
        event: Option<Event>,
    ) -> Result<(), RecordError> {
        self.log
            .record(&entry_for(self.turn_id, message_id, item.clone()))
            .await?;

        self.session.state().push(item);
        if let Some(event) = event {
            self.sink.emit(event);
        }
        Ok(())
    }

    /// Records a tool result and announces it, for a call already recorded.
    ///
    /// Separate from [`Journal::record`] only because the outcome is moved into the
    /// event while its output is cloned into the history entry — the two need the
    /// same value and only one of them can have it. Spelled out here rather than left
    /// to a `clone` at the call site, where the reason for the pair would not be.
    ///
    /// # Errors
    ///
    /// As [`Journal::record`].
    pub async fn record_result(
        &self,
        call_id: String,
        name: String,
        outcome: ToolOutcome,
    ) -> Result<(), RecordError> {
        self.record(
            None,
            ResponseItem::FunctionCallOutput {
                // Cloned rather than moved: the same id goes on the wire, and the
                // pair is what a consumer upserts on — a result whose id differed
                // from its request's would land as a separate call.
                call_id: call_id.clone(),
                output: outcome.output.clone(),
            },
            Some(Event::ToolCallEnd {
                call_id,
                name,
                result: outcome,
            }),
        )
        .await
    }

    /// Records the turn's opening and announces it.
    ///
    /// Announced as well as recorded, unlike the records that follow: the protocol
    /// has no `session/update` for a turn starting, so this event exists only to
    /// tell a consumer which turn the events after it belong to. Dropping it
    /// leaves every one of them unattributed.
    ///
    /// # Errors
    ///
    /// If the log refuses the record. Nothing has been announced at this point, so
    /// there is nothing to take back.
    pub async fn open_turn(&self) -> Result<(), RecordError> {
        self.log
            .record(&crate::session_event::SessionEvent::TurnStarted {
                turn_id: self.turn_id,
            })
            .await?;
        self.sink.emit(Event::TurnStarted {
            turn_id: self.turn_id,
        });
        Ok(())
    }

    /// Records the system instruction, which belongs to no turn.
    ///
    /// Recorded and never announced: the protocol has nowhere to put it, and a
    /// consumer has no use for the agent's own prefix. It goes in the log because
    /// a reader rebuilding a model request needs it, and the request cannot be
    /// rebuilt without it.
    ///
    /// Written above the first turn rather than inside it because it is the
    /// session's prefix, not part of any turn's conversation — and because a
    /// reader should not have to know that only the first turn's copy counts.
    ///
    /// # Errors
    ///
    /// If the log refuses the record. The turn has not opened and nothing has been
    /// announced, so refusing here leaves no trace to take back.
    pub async fn record_system(&self, content: &str) -> Result<(), RecordError> {
        self.log
            .record(&crate::session_event::SessionEvent::system(content))
            .await
    }

    /// Records the turn's end and then announces it.
    ///
    /// # Errors
    ///
    /// As [`Journal::record`]. A turn whose end cannot be recorded still ends —
    /// the caller falls back to announcing it without a record, which leaves the
    /// log one record short rather than leaving the turn open.
    pub async fn end(&self, reason: TurnEndReason) -> Result<(), RecordError> {
        self.log
            .record(&crate::session_event::SessionEvent::TurnEnded {
                turn_id: self.turn_id,
                reason,
            })
            .await?;
        Ok(())
    }

    /// The turn being recorded.
    #[must_use]
    pub fn turn_id(&self) -> TurnId {
        self.turn_id
    }

    /// The session whose history this journal extends.
    ///
    /// Read-only: the turn loop may look at the session to build the next model
    /// request, but only [`Journal::record`] writes to it.
    #[must_use]
    pub fn session(&self) -> &'a Session {
        self.session
    }

    /// The event sink.
    ///
    /// Exposed for **streamed** events only — the chunks of a message as they
    /// arrive. A chunk is not a record: it is a fragment of one, and there is
    /// nothing to write until the message closes. Anything that represents a
    /// whole history entry goes through [`Journal::record`] instead, which is what
    /// puts it on disk first.
    #[must_use]
    pub fn sink(&self) -> &'a dyn EventSink {
        self.sink
    }
}

/// Runs one turn to completion.
///
/// The loop owns sequencing; the caller supplies the pieces it needs.
///
/// # Errors
///
/// Returns [`TurnError::Busy`] if the session already has an active turn, and
/// [`TurnError::Unrecordable`] if the session's log refused a record the turn
/// could not do without. A turn that ends because the model failed is not an
/// error at this level: it reports [`TurnEndReason::Error`] through the terminator
/// event and the returned [`TurnResult`], because the turn did complete — it just
/// failed.
pub async fn run_turn(
    session: &Session,
    input: TurnInput,
    client: &dyn ModelClient,
    tools: &ToolRegistry,
    log: &dyn SessionLog,
    sink: &dyn EventSink,
) -> Result<TurnResult, TurnError> {
    let turn_id = TurnId::new();
    let guard = session.begin_turn().ok_or(TurnError::Busy)?;
    let cancel = guard.cancellation();
    let journal = Journal::new(session, log, sink, turn_id);

    // The sections are chosen here and handed down, rather than rebuilt inside
    // `drive`, so the instruction recorded and the instruction sent are rendered
    // from one value. Built in both places they would agree only until one of them
    // was changed, and a log holding a different prefix from the one sent is worse
    // than no log at all.
    let sections = prompt::default_sections();

    // Written once, above every turn. Emptiness is the test for whether this is
    // the first: nothing is recorded before the turn's own opening, so an empty
    // history means no turn has contributed an item yet. If a turn opens and then
    // fails to record an item, the history stays empty while the log already holds
    // a turn, and the next turn writes a second instruction — the reader takes the
    // last, and the bytes are the same either way.
    if journal.session().state().is_empty() {
        journal
            .record_system(&prompt::render_instructions(&sections))
            .await
            .map_err(TurnError::Unrecordable)?;
    }

    // Everything up to the model's first request is recorded before it is announced.
    // Any failure here leaves a turn that opened in the log and never closed,
    // which every later reader would treat as unfinished — so the two openings are
    // handled together, and a failure in either closes the turn before reporting.
    if let Err(err) = open(&journal, &input).await {
        // Nothing was announced, so nothing is announced here either: the caller
        // is told instead.
        let _ = journal.end(TurnEndReason::Error).await;
        return Err(TurnError::Unrecordable(err));
    }

    let reason = drive(&journal, client, tools, &cancel, &sections).await;

    // The end is announced even when it could not be recorded. A log one record
    // short is recoverable; a turn left open on the wire is not, because the
    // caller waits on this turn to close.
    let _ = journal.end(reason).await;

    sink.emit(Event::TurnComplete { turn_id, reason });

    Ok(TurnResult { turn_id, reason })
}

/// A turn's opening: the record that starts it, then what the user said.
///
/// The turn's own record is announced as well as written. It has no
/// `session/update` of its own — the protocol has no wire form for a turn starting
/// — but the sink needs the event to attribute everything after it: a consumer is
/// told which turn a chunk belongs to by way of this, and one that never arrives
/// leaves every later event unattributed and dropped.
///
/// The session's own record is **not** written here. It belongs to creating the
/// session, not to running a turn: a log that reopened with a header on every turn
/// would give a reader as many candidates for "where this session started" as the
/// session had turns.
///
/// # Errors
///
/// Fails if the log refuses any of the three records.
async fn open(journal: &Journal<'_>, input: &TurnInput) -> Result<(), RecordError> {
    journal.open_turn().await?;

    // Where and when this turn runs is recorded ahead of the user's own words, so
    // the model reads its context before the request it has to answer. No event is
    // emitted for it: it is context, not something the user said.
    journal
        .record(None, context::item(journal.session().cwd()), None)
        .await?;

    // The user's input is recorded next so every later request carries it.
    journal
        .record(
            None,
            prompt::user_item(&input.text),
            Some(Event::UserMessage {
                content: input.text.clone(),
            }),
        )
        .await
}

/// The loop proper, separated so the caller always emits a terminator.
async fn drive(
    journal: &Journal<'_>,
    client: &dyn ModelClient,
    tools: &ToolRegistry,
    cancel: &tokio_util::sync::CancellationToken,
    sections: &[prompt::Section],
) -> TurnEndReason {
    loop {
        if cancel.is_cancelled() {
            return TurnEndReason::Interrupted;
        }

        let request = {
            let state = journal.session.state();
            prompt::build_prompt(&state, tools.definitions(), Some(sections))
        };

        match run_step(journal, client, tools, cancel, request).await {
            Ok(StepOutcome::Finished) => return TurnEndReason::Completed,
            // Tool output is now in history, so another request is owed.
            Ok(StepOutcome::NeedsFollowUp) => continue,
            Err(reason) => return reason,
        }
    }
}

/// How one model round-trip ended.
enum StepOutcome {
    /// The model produced no tool calls, so nothing is outstanding.
    Finished,
    /// Tool output was recorded and another request is owed.
    NeedsFollowUp,
}

/// Runs one request/response round-trip, executing any tools it asked for.
async fn run_step(
    journal: &Journal<'_>,
    client: &dyn ModelClient,
    tools: &ToolRegistry,
    cancel: &tokio_util::sync::CancellationToken,
    request: ModelRequest,
) -> Result<StepOutcome, TurnEndReason> {
    let mut stream = match client.stream(request, cancel.clone()).await {
        Ok(stream) => stream,
        Err(_) => return Err(TurnEndReason::Error),
    };

    // One id per message, minted before the first chunk of it. Every chunk of this
    // message repeats it, so a consumer can tell where one message ends and the
    // next begins — which it cannot do from the text alone, since a delta carries
    // no boundary.
    let message_id = MessageId::new();
    // Reasoning is a separate message: it arrives as its own stream and a client
    // renders it apart from the answer, so sharing an id would group two things
    // the consumer already distinguishes.
    let thought_id = MessageId::new();

    // Accumulated so one recorded item closes the assistant message however
    // many chunks it arrived in.
    let mut message = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<PendingToolCall> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            // The partial message is flushed either way — a turn that was cut off
            // still said something, and the log should hold it. A refused flush
            // does not change why the turn is ending, and reporting it here would
            // report the interruption as something else.
            let _ = flush_assistant(journal, &message_id, &thought_id, &message, &reasoning).await;
            return Err(TurnEndReason::Interrupted);
        }

        let next = stream.next().await;
        let Some(item) = next else {
            // The provider went away without signalling completion.
            let _ = flush_assistant(journal, &message_id, &thought_id, &message, &reasoning).await;
            return Err(TurnEndReason::Error);
        };

        let event = match item {
            Ok(event) => event,
            Err(_) => {
                let _ =
                    flush_assistant(journal, &message_id, &thought_id, &message, &reasoning).await;
                return Err(TurnEndReason::Error);
            }
        };

        match event {
            ModelEvent::TextDelta { delta } => {
                message.push_str(&delta);
                journal.sink().emit(Event::AgentMessageDelta {
                    message_id: Some(message_id.clone()),
                    delta,
                });
            }
            ModelEvent::ThoughtDelta { delta } => {
                reasoning.push_str(&delta);
                journal.sink().emit(Event::AgentThoughtDelta {
                    message_id: Some(thought_id.clone()),
                    delta,
                });
            }
            ModelEvent::ToolCall {
                call_id,
                name,
                arguments,
            } => tool_calls.push(PendingToolCall {
                call_id,
                name,
                arguments,
            }),
            ModelEvent::Done => break,
        }
    }

    // Not ignored here, unlike the interrupted and errored paths above: this is the
    // turn's own output, and a log that refused it means the model answered into
    // nothing. Continuing would leave the turn looking answered on the wire and
    // unfinished in the log.
    flush_assistant(journal, &message_id, &thought_id, &message, &reasoning)
        .await
        .map_err(|_| TurnEndReason::Error)?;

    if tool_calls.is_empty() {
        return Ok(StepOutcome::Finished);
    }

    for call in tool_calls {
        if cancel.is_cancelled() {
            return Err(TurnEndReason::Interrupted);
        }
        if run_tool_call(journal, tools, call).await.is_err() {
            return Err(TurnEndReason::Error);
        }
    }

    Ok(StepOutcome::NeedsFollowUp)
}

/// A tool call the model asked for, not yet executed.
struct PendingToolCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Records the assistant's reasoning and message, if either was produced.
///
/// **Reasoning first**, because that is the order they arrived in: a model thinks,
/// then answers, and a log that records the answer first describes a conversation
/// that did not happen.
///
/// The order is not cosmetic. `session/load` replays a log by walking it, and the
/// consumer opens a step from whichever chunk arrives first — so a log with the
/// answer before the thinking renders the reply above the reasoning that produced
/// it.
///
/// Reasoning is also never sent back to the model (`prompt::is_model_visible`), so
/// its position here cannot affect what the model sees on the next request. There
/// was nothing to preserve by recording it late.
///
/// # Errors
///
/// Fails if either record is refused. The caller treats that as the turn failing:
/// half an answer recorded is not an answer.
async fn flush_assistant(
    journal: &Journal<'_>,
    message_id: &MessageId,
    thought_id: &MessageId,
    message: &str,
    reasoning: &str,
) -> Result<(), RecordError> {
    if !reasoning.is_empty() {
        journal
            .record(
                Some(thought_id.clone()),
                ResponseItem::Reasoning {
                    content: reasoning.to_owned(),
                },
                None,
            )
            .await?;
    }
    if !message.is_empty() {
        journal
            .record(
                Some(message_id.clone()),
                ResponseItem::Message {
                    role: Role::Assistant,
                    content: message.to_owned(),
                },
                None,
            )
            .await?;
    }
    Ok(())
}

/// Executes one tool call and records its output.
///
/// # Errors
///
/// Fails if the call or its result could not be recorded. The tool itself does not
/// fail this way: a tool that ran and failed returns an outcome, and that outcome
/// is recorded like any other.
async fn run_tool_call(
    journal: &Journal<'_>,
    tools: &ToolRegistry,
    call: PendingToolCall,
) -> Result<(), RecordError> {
    let arguments: serde_json::Value =
        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);

    // The request is recorded before the result, so the pair stays adjacent.
    // Recorded before the tool runs too, so a log cut short by a crash still says
    // what was attempted — an unanswered call is the one shape a model API will
    // reject on resume.
    journal
        .record(
            None,
            ResponseItem::FunctionCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            },
            Some(Event::ToolCallBegin {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: arguments.clone(),
            }),
        )
        .await?;

    let ctx = ToolContext {
        cwd: journal.session().cwd().to_path_buf(),
    };
    let outcome = match tools.dispatch(&call.name, &ctx, arguments).await {
        Ok(outcome) => outcome,
        Err(ToolError::NotFound(name)) => ToolOutcome::failure(failure_text(
            format!("There is no tool named `{name}`."),
            Some("Call one of the tools listed above. Nothing was run."),
        )),
        Err(err) => ToolOutcome::failure(failure_text(
            format!("The arguments were rejected before anything ran: {err}"),
            Some("Nothing was run. Fix the arguments and call the tool again."),
        )),
    };

    journal
        .record_result(call.call_id, call.name, outcome)
        .await
}

/// A turn could not be started, or could not be recorded.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// The session already has an active turn.
    #[error("session already has an active turn")]
    Busy,

    /// The session's log refused a record the turn needed.
    ///
    /// The turn stops rather than continuing unrecorded. Carrying on would let the
    /// model read history the log does not have, which is the one state that
    /// cannot be recovered from: there is no way to tell, later, which of the two
    /// is the truth.
    #[error("the session's log refused a record: {0}")]
    Unrecordable(#[source] RecordError),
}
