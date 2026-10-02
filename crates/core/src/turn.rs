//! The turn loop.
//!
//! [`run_turn`] is the only writer of session history and the only emitter of
//! [`Event`]s. Every exit path funnels through a single `TurnComplete`, so a
//! turn always ends exactly once, and no other code competes to emit.

use futures::StreamExt;

use crate::client::{ModelClient, ModelEvent, ModelRequest};
use crate::context;
use crate::prompt;
use crate::session::Session;
use crate::tools::{ToolContext, ToolError, ToolOutcome, ToolRegistry};
use crate::types::{Event, EventSink, ResponseItem, Role, TurnEndReason, TurnId, TurnInput};

/// What a finished turn produced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnResult {
    /// Which turn this was.
    pub turn_id: TurnId,
    /// Why it stopped.
    pub reason: TurnEndReason,
}

/// Runs one turn to completion.
///
/// The loop owns sequencing; the caller supplies the pieces it needs.
///
/// # Errors
///
/// Returns [`TurnError::Busy`] if the session already has an active turn. A
/// turn that ends because the model failed is not an error at this level: it
/// reports [`TurnEndReason::Error`] through the terminator event and the
/// returned [`TurnResult`], because the turn did complete — it just failed.
pub async fn run_turn(
    session: &Session,
    input: TurnInput,
    client: &dyn ModelClient,
    tools: &ToolRegistry,
    sink: &dyn EventSink,
) -> Result<TurnResult, TurnError> {
    let turn_id = TurnId::new();
    let guard = session.begin_turn().ok_or(TurnError::Busy)?;
    let cancel = guard.cancellation();

    sink.emit(Event::TurnStarted { turn_id });

    // Where and when this turn runs is recorded ahead of the user's own words,
    // so the model reads its context before the request it has to answer. No
    // event is emitted for it: it is context, not something the user said.
    session.state().push(context::item(session.cwd()));

    // The user's input is recorded next so every later request carries it.
    let user_item = prompt::user_item(&input.text);
    session.state().push(user_item.clone());
    sink.emit(Event::UserMessage {
        content: input.text.clone(),
    });

    let reason = drive(session, client, tools, sink, &cancel).await;

    sink.emit(Event::TurnComplete { turn_id, reason });

    Ok(TurnResult { turn_id, reason })
}

/// The loop proper, separated so the caller always emits a terminator.
async fn drive(
    session: &Session,
    client: &dyn ModelClient,
    tools: &ToolRegistry,
    sink: &dyn EventSink,
    cancel: &tokio_util::sync::CancellationToken,
) -> TurnEndReason {
    loop {
        if cancel.is_cancelled() {
            return TurnEndReason::Interrupted;
        }

        let request = {
            let state = session.state();
            prompt::build_prompt(&state, tools.definitions(), None)
        };

        match run_step(session, client, tools, sink, cancel, request).await {
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
    session: &Session,
    client: &dyn ModelClient,
    tools: &ToolRegistry,
    sink: &dyn EventSink,
    cancel: &tokio_util::sync::CancellationToken,
    request: ModelRequest,
) -> Result<StepOutcome, TurnEndReason> {
    let mut stream = match client.stream(request, cancel.clone()).await {
        Ok(stream) => stream,
        Err(_) => return Err(TurnEndReason::Error),
    };

    // Accumulated so one recorded item closes the assistant message however
    // many chunks it arrived in.
    let mut message = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: Vec<PendingToolCall> = Vec::new();

    loop {
        if cancel.is_cancelled() {
            flush_assistant(session, &message, &reasoning);
            return Err(TurnEndReason::Interrupted);
        }

        let next = stream.next().await;
        let Some(item) = next else {
            // The provider went away without signalling completion.
            flush_assistant(session, &message, &reasoning);
            return Err(TurnEndReason::Error);
        };

        let event = match item {
            Ok(event) => event,
            Err(_) => {
                flush_assistant(session, &message, &reasoning);
                return Err(TurnEndReason::Error);
            }
        };

        match event {
            ModelEvent::TextDelta { delta } => {
                message.push_str(&delta);
                sink.emit(Event::AgentMessageDelta { delta });
            }
            ModelEvent::ThoughtDelta { delta } => {
                reasoning.push_str(&delta);
                sink.emit(Event::AgentThoughtDelta { delta });
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

    flush_assistant(session, &message, &reasoning);

    if tool_calls.is_empty() {
        return Ok(StepOutcome::Finished);
    }

    for call in tool_calls {
        if cancel.is_cancelled() {
            return Err(TurnEndReason::Interrupted);
        }
        run_tool_call(session, tools, sink, call).await;
    }

    Ok(StepOutcome::NeedsFollowUp)
}

/// A tool call the model asked for, not yet executed.
struct PendingToolCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Records the assistant's message and reasoning, if any was produced.
fn flush_assistant(session: &Session, message: &str, reasoning: &str) {
    let mut state = session.state();
    if !message.is_empty() {
        state.push(ResponseItem::Message {
            role: Role::Assistant,
            content: message.to_owned(),
        });
    }
    if !reasoning.is_empty() {
        state.push(ResponseItem::Reasoning {
            content: reasoning.to_owned(),
        });
    }
}

/// Executes one tool call and records its output.
async fn run_tool_call(
    session: &Session,
    tools: &ToolRegistry,
    sink: &dyn EventSink,
    call: PendingToolCall,
) {
    let arguments: serde_json::Value =
        serde_json::from_str(&call.arguments).unwrap_or(serde_json::Value::Null);

    // The request is recorded before the result, so the pair stays adjacent.
    session.state().push(ResponseItem::FunctionCall {
        call_id: call.call_id.clone(),
        name: call.name.clone(),
        arguments: call.arguments.clone(),
    });

    sink.emit(Event::ToolCallBegin {
        call_id: call.call_id.clone(),
        name: call.name.clone(),
        arguments: arguments.clone(),
    });

    let ctx = ToolContext {
        cwd: session.cwd().to_path_buf(),
    };
    let outcome = match tools.dispatch(&call.name, &ctx, arguments).await {
        Ok(outcome) => outcome,
        Err(ToolError::NotFound(name)) => ToolOutcome::failure(format!("unknown tool: {name}")),
        Err(err) => ToolOutcome::failure(err.to_string()),
    };

    session.state().push(ResponseItem::FunctionCallOutput {
        call_id: call.call_id.clone(),
        output: outcome.output.clone(),
    });

    sink.emit(Event::ToolCallEnd {
        call_id: call.call_id,
        name: call.name,
        result: outcome,
    });
}

/// A turn could not be started.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// The session already has an active turn.
    #[error("session already has an active turn")]
    Busy,
}
