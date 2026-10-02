//! Core runtime for SrudAgent.
//!
//! Transport-agnostic: it consumes [`Op`] values and emits [`Event`] values,
//! with no knowledge of any wire protocol. Mapping [`Event`] onto a specific
//! protocol belongs to the layer above.
//!
//! # Layout
//!
//! - [`types`] — the vocabulary: [`Op`], [`Event`], [`ResponseItem`].
//! - [`session`] — session state and the active turn control block.
//! - [`context`] — the env-context block recorded ahead of each turn.
//! - [`tools`] — the [`Tool`](tools::Tool) trait and the registry.
//! - [`client`] — the model seam: [`ModelClient`](client::ModelClient).
//! - [`prompt`] — building a model request from session history.
//! - [`turn`] — the turn loop that ties everything together.

pub mod client;
pub mod context;
pub mod prompt;
pub mod session;
pub mod tools;
pub mod turn;
pub mod types;

pub use session::{ActiveTurnGuard, Session, SessionState};
pub use tools::{DuplicateTool, Tool, ToolDefinition, ToolError, ToolOutcome, ToolRegistry};
pub use turn::{run_turn, TurnError, TurnResult};
pub use types::{
    Event, EventSink, Op, ResponseItem, Role, SessionId, TurnEndReason, TurnId, TurnInput,
};
