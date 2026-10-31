//! Thinking-strength selection, in the vocabulary a model adapter consumes.
//!
//! The level a user chooses is model-agnostic; the value an endpoint accepts is
//! not. [`ThinkingRequest`] carries both: `value` is what this endpoint expects
//! for the selection, and [`off`](ThinkingRequest::off) says whether it means
//! "do not think" — which the request writes as a disabled toggle rather than
//! as an effort value.

/// One resolved thinking selection, ready for an adapter to put on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingRequest {
    /// The endpoint's value for this selection, as configured by the user.
    pub value: String,
    /// Whether the selection means the model should not think at all.
    pub off: bool,
}
