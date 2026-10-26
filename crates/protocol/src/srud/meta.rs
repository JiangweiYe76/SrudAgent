//! SrudAgent's private payload carried in the standard `_meta` field.
//!
//! ACP has no turn/step concept on the wire, but SrudAgent keeps them as
//! first-class domain concepts (interrupt targeting, rollout indexing, audit).
//! They ride along inside `_meta.srud` so third-party ACP clients ignore them
//! while SrudAgent's own frontend reads them.
//!
//! Two rules from the protocol contract are enforced here:
//!
//! 1. Custom data lives only under the `srud` key of `_meta` — never at the
//!    root of a standard object.
//! 2. The root keys `traceparent`, `tracestate`, `baggage` are reserved for
//!    W3C trace context and must not be occupied.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::acp::Meta;

/// The `_meta` key under which all SrudAgent private data is nested.
pub const SRUD_META_KEY: &str = "srud";

/// Root `_meta` keys reserved for W3C trace context. SrudAgent must not write
/// these; [`SrudMeta::insert_into`] rejects them.
pub const RESERVED_TRACE_KEYS: &[&str] = &["traceparent", "tracestate", "baggage"];

/// SrudAgent's turn/step metadata, serialised under `_meta.srud`.
///
/// Every field is optional: a payload carries only what its message needs.
/// Keys are camelCase on the wire (ACP convention).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrudMeta {
    /// The domain turn id. Required to target a `session/cancel` and to index
    /// the rollout, since ACP v1 has no turn id of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    /// Zero-based index of the current step within the turn (diagnostics only;
    /// `step` is not a protocol concept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u32>,
    /// Why a tool call failed, when `status: "failed"` is not specific enough
    /// (e.g. `sandbox_denied`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<String>,
    /// Whether the turn was interrupted. Mirrors `StopReason::Cancelled` but
    /// can be attached to individual updates for UI state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted: Option<bool>,
    /// The SrudAgent turn-end reason when the standard `stopReason` does not carry
    /// it — currently only `"blocked"` (approval denied), which maps to
    /// `stopReason: "end_turn"` plus this marker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_end_reason: Option<String>,
    /// A limit that ended the turn, when `stopReason` is `max_tokens` /
    /// `max_turn_requests` but SrudAgent reports it as a completed turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<String>,
    /// Whether the model refused, when reported as a completed turn with
    /// `stopReason: "end_turn"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<bool>,
    /// Token counts for the step that just ran, breaking down what the stable
    /// `usage` fields aggregate. Absent when the provider reported no usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<SrudUsage>,
    /// Every turn the session has finished and why. Carried on the `session/load`
    /// response, which is where a client learns how each turn of a replayed
    /// conversation ended. A turn the list leaves out is one still running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turns: Option<Vec<SrudTurnEnd>>,
}

/// One finished turn, as `_meta.srud.turns` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrudTurnEnd {
    /// The turn's id, the same one every update of it carried.
    pub turn_id: String,
    /// `completed`, `interrupted`, `blocked` or `error`.
    pub end_reason: String,
}

/// Per-step token counts carried under `_meta.srud.usage`.
///
/// The standard `UsageUpdate` only has `used` and `size`; the breakdown lives
/// here so third-party clients still parse the stable fields while SrudAgent's
/// own frontend can show cache hits and reasoning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SrudUsage {
    /// Tokens sent to the model for this step.
    #[serde(default)]
    pub input_tokens: u64,
    /// Tokens the model produced for this step.
    #[serde(default)]
    pub output_tokens: u64,
    /// Input tokens served from the provider's cache.
    #[serde(default)]
    pub cached_input_tokens: u64,
    /// Input tokens written into the cache (Responses API only).
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    /// Output tokens spent on reasoning.
    #[serde(default)]
    pub reasoning_output_tokens: u64,
}

impl SrudMeta {
    /// Returns `true` when there is nothing to attach.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turn_id.is_none()
            && self.step_index.is_none()
            && self.failure_kind.is_none()
            && self.interrupted.is_none()
            && self.turn_end_reason.is_none()
            && self.limit.is_none()
            && self.refusal.is_none()
            && self.usage.is_none()
            && self.turns.is_none()
    }

    /// Wraps this payload as a standalone `_meta` object: `{"srud": {...}}`.
    ///
    /// Returns `None` when the payload is empty, so callers can skip attaching
    /// `_meta` entirely instead of emitting `{"srud":{}}`.
    #[must_use]
    pub fn to_meta(&self) -> Option<Meta> {
        if self.is_empty() {
            return None;
        }
        let inner = serde_json::to_value(self)
            .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
        let mut meta = Meta::new();
        meta.insert(SRUD_META_KEY.to_string(), inner);
        Some(meta)
    }

    /// Extracts the `srud` sub-object from a `_meta` value, if present and
    /// well-formed. Absent or malformed data yields `None` — callers must not
    /// assume the shape of another implementation's `_meta.srud`.
    #[must_use]
    pub fn from_meta(meta: &Meta) -> Option<Self> {
        meta.get(SRUD_META_KEY)
            .and_then(|value| serde_json::from_value(value.clone()).ok())
    }

    /// Inserts this payload's `srud` key into an existing `_meta`, preserving
    /// any other keys the peer already set.
    ///
    /// Returns an error if `meta` already occupies a reserved trace key at the
    /// root — those belong to W3C trace context and must not be overwritten.
    pub fn insert_into(&self, meta: &mut Meta) -> Result<(), ReservedKeyConflict> {
        if let Some(key) = find_reserved(meta.keys()) {
            return Err(ReservedKeyConflict(key.to_string()));
        }
        if self.is_empty() {
            return Ok(());
        }
        let inner = serde_json::to_value(self)
            .unwrap_or_else(|_| serde_json::Value::Object(Default::default()));
        meta.insert(SRUD_META_KEY.to_string(), inner);
        Ok(())
    }
}

/// Error returned when an operation would touch a reserved W3C trace key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReservedKeyConflict(pub String);

impl std::fmt::Display for ReservedKeyConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`_meta` key `{}` is reserved for W3C trace context and must not be written",
            self.0
        )
    }
}

impl std::error::Error for ReservedKeyConflict {}

fn find_reserved<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a String>,
{
    let reserved: BTreeSet<&str> = RESERVED_TRACE_KEYS.iter().copied().collect();
    keys.into_iter()
        .map(String::as_str)
        .find(|key| reserved.contains(key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_meta_produces_no_wrapper() {
        assert_eq!(SrudMeta::default().to_meta(), None);
    }

    #[test]
    fn serialises_under_srud_key_with_camel_case() {
        let meta = SrudMeta {
            turn_id: Some("turn_01".to_string()),
            step_index: Some(3),
            ..Default::default()
        };
        let value = serde_json::to_value(meta.to_meta().unwrap()).unwrap();
        assert_eq!(
            value,
            json!({ "srud": { "turnId": "turn_01", "stepIndex": 3 } })
        );
    }

    #[test]
    fn round_trips_through_meta() {
        let original = SrudMeta {
            turn_id: Some("t".into()),
            failure_kind: Some("sandbox_denied".into()),
            interrupted: Some(true),
            refusal: Some(false),
            ..Default::default()
        };
        let meta = original.to_meta().unwrap();
        let parsed = SrudMeta::from_meta(&meta).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn a_turn_list_survives_the_wrapper() {
        let meta = SrudMeta {
            turns: Some(vec![
                SrudTurnEnd {
                    turn_id: "t1".into(),
                    end_reason: "completed".into(),
                },
                SrudTurnEnd {
                    turn_id: "t2".into(),
                    end_reason: "interrupted".into(),
                },
            ]),
            ..Default::default()
        };

        // Only `turns` is set: the list alone makes the payload worth wrapping.
        let wrapped = meta.to_meta().expect("a turn list is not empty");
        let value = serde_json::to_value(&wrapped).unwrap();
        assert_eq!(
            value,
            json!({ "srud": { "turns": [
                { "turnId": "t1", "endReason": "completed" },
                { "turnId": "t2", "endReason": "interrupted" },
            ] } })
        );
        assert_eq!(SrudMeta::from_meta(&wrapped), Some(meta));
    }

    #[test]
    fn from_meta_ignores_absent_or_malformed() {
        let mut meta = Meta::new();
        meta.insert("other".to_string(), json!(1));
        assert_eq!(SrudMeta::from_meta(&meta), None);
        meta.insert(SRUD_META_KEY.to_string(), json!("not an object"));
        assert_eq!(SrudMeta::from_meta(&meta), None);
    }

    #[test]
    fn insert_into_preserves_other_keys() {
        let mut meta = Meta::new();
        meta.insert("someone".to_string(), json!("else"));
        SrudMeta {
            turn_id: Some("t".into()),
            ..Default::default()
        }
        .insert_into(&mut meta)
        .unwrap();
        assert!(meta.contains_key("someone"));
        assert!(meta.contains_key(SRUD_META_KEY));
    }

    #[test]
    fn insert_into_rejects_reserved_trace_keys() {
        let mut meta = Meta::new();
        meta.insert("traceparent".to_string(), json!("00-abc"));
        let err = SrudMeta::default().insert_into(&mut meta).unwrap_err();
        assert_eq!(err.0, "traceparent");
    }
}
