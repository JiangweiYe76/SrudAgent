//! The turn-end reason model and its mapping onto ACP v1's `StopReason`.
//!
//! SrudAgent's turn ends for four reasons. ACP v1's
//! `StopReason` covers two of them directly; the other two need a wire
//! representation:
//!
//! | SrudAgent reason | `stopReason`      | extra signal                       |
//! |------------------|-------------------|------------------------------------|
//! | `Completed`      | `end_turn`        | —                                  |
//! | `Interrupted`    | `cancelled`       | —                                  |
//! | `Blocked`        | `end_turn`        | `_meta.srud.turnEndReason="blocked"` |
//! | `Error`          | *no result*       | JSON-RPC error response            |
//!
//! `Error` is deliberately absent from [`map_turn_end_reason`]: an errored
//! turn answers `session/prompt` with a JSON-RPC error, not a result, so
//! there is no `StopReason` to map to. The server layer handles that
//! path; this module only covers the three reasons that end with a result.

use crate::acp::StopReason;
use crate::srud::meta::SrudMeta;

/// The `_meta.srud.turnEndReason` value marking an approval-blocked turn.
pub const TURN_END_REASON_BLOCKED: &str = "blocked";

/// Why a SrudAgent turn ended, as the domain sees it.
///
/// This mirrors the core runtime's notion of turn termination; the server
/// converts between the two. It lives here (not imported from core) because
/// the protocol crate sits at the bottom of the dependency graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrudTurnEndReason {
    /// The model finished the turn normally.
    Completed,
    /// The client cancelled the turn via `session/cancel`.
    Interrupted,
    /// The turn ended because an approval was denied — a normal business
    /// outcome, not a protocol error.
    Blocked,
}

/// The wire shape of a turn that ended with a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnEndWire {
    /// The standard stop reason to return on `session/prompt`.
    pub stop_reason: StopReason,
    /// The `_meta.srud` payload to attach, when the reason diverges from the
    /// standard vocabulary.
    pub meta: Option<SrudMeta>,
}

/// Maps a [`SrudTurnEndReason`] onto its ACP v1 wire representation.
#[must_use]
pub fn map_turn_end_reason(reason: SrudTurnEndReason) -> TurnEndWire {
    match reason {
        SrudTurnEndReason::Completed => TurnEndWire {
            stop_reason: StopReason::EndTurn,
            meta: None,
        },
        SrudTurnEndReason::Interrupted => TurnEndWire {
            stop_reason: StopReason::Cancelled,
            meta: None,
        },
        SrudTurnEndReason::Blocked => TurnEndWire {
            stop_reason: StopReason::EndTurn,
            meta: Some(SrudMeta {
                turn_end_reason: Some(TURN_END_REASON_BLOCKED.to_string()),
                ..Default::default()
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_maps_to_end_turn_without_meta() {
        let wire = map_turn_end_reason(SrudTurnEndReason::Completed);
        assert_eq!(wire.stop_reason, StopReason::EndTurn);
        assert_eq!(wire.meta, None);
    }

    #[test]
    fn interrupted_maps_to_cancelled_without_meta() {
        let wire = map_turn_end_reason(SrudTurnEndReason::Interrupted);
        assert_eq!(wire.stop_reason, StopReason::Cancelled);
        assert_eq!(wire.meta, None);
    }

    #[test]
    fn blocked_maps_to_end_turn_with_meta_marker() {
        let wire = map_turn_end_reason(SrudTurnEndReason::Blocked);
        assert_eq!(wire.stop_reason, StopReason::EndTurn);
        let meta = wire.meta.expect("blocked carries meta");
        assert_eq!(
            meta.turn_end_reason.as_deref(),
            Some(TURN_END_REASON_BLOCKED)
        );
        // The marker must survive the `_meta` wrapping round-trip.
        let wrapped = meta.to_meta().expect("non-empty");
        let parsed = SrudMeta::from_meta(&wrapped).unwrap();
        assert_eq!(parsed.turn_end_reason.as_deref(), Some("blocked"));
    }

    #[test]
    fn stop_reason_serialises_snake_case() {
        assert_eq!(
            serde_json::to_value(StopReason::EndTurn).unwrap(),
            serde_json::json!("end_turn")
        );
        assert_eq!(
            serde_json::to_value(StopReason::Cancelled).unwrap(),
            serde_json::json!("cancelled")
        );
    }
}
