//! The core vocabulary: inputs, outputs, and the recorded history.
//!
//! - [`Op`] — what callers ask the runtime to do.
//! - [`Event`] — what the runtime reports as it works.
//! - [`ResponseItem`] — what the model saw.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identifies a session. Stable across process restarts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

/// Identifies one turn within a session.
///
/// A turn is one user input and everything the agent does in response. It is
/// not a wire concept: it exists so that interrupts, audit records, and history
/// indexes have something stable to point at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId(pub Uuid);

impl SessionId {
    /// Creates a fresh, random session id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

impl TurnId {
    /// Creates a fresh, random turn id.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TurnId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::fmt::Display for TurnId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A request to the runtime.
#[derive(Debug, Clone)]
pub enum Op {
    /// Process a user input as one turn.
    TurnInput(TurnInput),
    /// Cancel the active turn, if any.
    Interrupt,
    /// Compress history to reclaim context budget.
    Compact,
    /// Stop accepting work and wind down.
    Shutdown,
}

/// A single unit of user input.
#[derive(Debug, Clone)]
pub struct TurnInput {
    /// The user's text.
    pub text: String,
}

/// Why a turn stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndReason {
    /// The model produced a final answer and nothing is outstanding.
    Completed,
    /// The turn was cancelled.
    Interrupted,
    /// A required approval was denied, so work could not continue.
    Blocked,
    /// An internal failure (transport error, retry budget exhausted, ...).
    Error,
}

/// An observable step in a turn.
///
/// Emitted in order and never replayed by this layer.
#[derive(Debug, Clone)]
pub enum Event {
    /// A turn began.
    TurnStarted { turn_id: TurnId },
    /// The user's input, echoed back so a consumer can render it in order.
    UserMessage { content: String },
    /// A chunk of assistant-visible text.
    AgentMessageDelta { delta: String },
    /// A chunk of reasoning text, if the model exposes it.
    AgentThoughtDelta { delta: String },
    /// A tool call is about to run.
    ToolCallBegin {
        call_id: String,
        name: String,
        arguments: serde_json::Value,
    },
    /// A tool call finished.
    ToolCallEnd {
        call_id: String,
        name: String,
        result: crate::tools::ToolOutcome,
    },
    /// The turn stopped. Exactly one of these closes every turn.
    TurnComplete {
        turn_id: TurnId,
        reason: TurnEndReason,
    },
}

/// A sink for [`Event`] values.
pub trait EventSink: Send + Sync {
    /// Records one event. Implementations must not block for long.
    fn emit(&self, event: Event);
}

/// An [`EventSink`] that discards everything.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: Event) {}
}

/// A role in the conversation, from the model's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The end user.
    User,
    /// The model.
    Assistant,
    /// Tool output fed back to the model.
    Tool,
}

/// One entry in the recorded history.
///
/// History is the source of truth. The turn loop is the only writer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseItem {
    /// A conversational message.
    Message { role: Role, content: String },
    /// The model asking for a tool to run.
    FunctionCall {
        call_id: String,
        name: String,
        /// Raw JSON, kept as a string so it round-trips losslessly.
        arguments: String,
    },
    /// The result of a tool call, fed back to the model.
    FunctionCallOutput { call_id: String, output: String },
    /// Model reasoning, when the provider returns it separately.
    Reasoning { content: String },
}

impl ResponseItem {
    /// Builds a user message item.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self::Message {
            role: Role::User,
            content: content.into(),
        }
    }

    /// Builds an assistant message item.
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::Message {
            role: Role::Assistant,
            content: content.into(),
        }
    }

    /// Returns the call id if this item refers to a tool call.
    #[must_use]
    pub fn call_id(&self) -> Option<&str> {
        match self {
            Self::FunctionCall { call_id, .. } | Self::FunctionCallOutput { call_id, .. } => {
                Some(call_id)
            }
            Self::Message { .. } | Self::Reasoning { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_item_round_trips_through_json() {
        let items = vec![
            ResponseItem::user("hello"),
            ResponseItem::assistant("hi"),
            ResponseItem::FunctionCall {
                call_id: "call_1".into(),
                name: "read_file".into(),
                arguments: r#"{"path":"/tmp/x"}"#.into(),
            },
            ResponseItem::FunctionCallOutput {
                call_id: "call_1".into(),
                output: "contents".into(),
            },
            ResponseItem::Reasoning {
                content: "thinking".into(),
            },
        ];

        for item in items {
            let json = serde_json::to_string(&item).expect("serializes");
            let back: ResponseItem = serde_json::from_str(&json).expect("deserializes");
            assert_eq!(
                serde_json::to_string(&back).expect("re-serializes"),
                json,
                "item did not round-trip: {json}"
            );
        }
    }

    #[test]
    fn call_id_is_exposed_only_for_tool_items() {
        let call = ResponseItem::FunctionCall {
            call_id: "call_9".into(),
            name: "shell".into(),
            arguments: "{}".into(),
        };
        assert_eq!(call.call_id(), Some("call_9"));
        assert_eq!(ResponseItem::user("hi").call_id(), None);
    }
}
