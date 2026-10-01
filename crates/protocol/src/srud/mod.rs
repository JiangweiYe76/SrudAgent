//! SrudAgent's private extensions on top of ACP.
//!
//! Standard ACP types are re-exported from [`crate::acp`]; this module holds
//! everything SrudAgent adds: the `_meta.srud` payload ([`meta`]), the
//! `_srud/unstable/*` method types ([`methods`]), and the turn-end reason
//! mapping ([`turn_end`]).

pub mod meta;
pub mod methods;
pub mod turn_end;

use serde::{Deserialize, Serialize};

use crate::acp::SessionUpdate;

/// A `session/update` that tolerates variants this build does not know.
///
/// ACP evolves by adding `sessionUpdate` variants, and the compatibility rule
/// is that unknown notifications are silently ignored.
/// The upstream [`SessionUpdate`] is `#[non_exhaustive]` and internally tagged,
/// so deserialising a future variant into it fails outright. Consumers that
/// must survive that — the client bridge, above all — parse into this wrapper
/// instead: known variants land in [`MaybeSessionUpdate::Known`], anything
/// else is preserved verbatim in [`MaybeSessionUpdate::Unknown`] and can be
/// dropped or logged without breaking the stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MaybeSessionUpdate {
    /// A variant this build understands. Boxed because [`SessionUpdate`] is
    /// far larger than the raw-JSON fallback and these values are relayed,
    /// not pattern-matched in hot paths.
    Known(Box<SessionUpdate>),
    /// A variant from a newer protocol revision: kept as raw JSON so it can be
    /// re-serialised unchanged (e.g. when relaying to another peer).
    Unknown(serde_json::Value),
}

impl MaybeSessionUpdate {
    /// Returns the parsed update, or `None` for an unknown variant.
    #[must_use]
    pub fn as_update(&self) -> Option<&SessionUpdate> {
        match self {
            Self::Known(update) => Some(&**update),
            Self::Unknown(_) => None,
        }
    }

    /// Returns `true` when the variant is not understood by this build.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn known_variant_parses() {
        let value = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": { "type": "text", "text": "hi" }
        });
        let parsed: MaybeSessionUpdate = serde_json::from_value(value).unwrap();
        assert!(!parsed.is_unknown());
        assert!(matches!(
            parsed.as_update(),
            Some(SessionUpdate::AgentMessageChunk(_))
        ));
    }

    #[test]
    fn unknown_variant_is_preserved_verbatim() {
        let value = json!({
            "sessionUpdate": "some_future_update",
            "payload": { "x": 1 }
        });
        let parsed: MaybeSessionUpdate = serde_json::from_value(value.clone()).unwrap();
        assert!(parsed.is_unknown());
        assert_eq!(parsed.as_update(), None);
        // Re-serialising must reproduce the original JSON exactly.
        assert_eq!(serde_json::to_value(&parsed).unwrap(), value);
    }

    #[test]
    fn known_variant_round_trips() {
        let update = SessionUpdate::AgentMessageChunk(crate::acp::ContentChunk::new(
            crate::acp::ContentBlock::Text(crate::acp::TextContent::new("hi")),
        ));
        let wrapped = MaybeSessionUpdate::Known(Box::new(update.clone()));
        let value = serde_json::to_value(&wrapped).unwrap();
        let parsed: MaybeSessionUpdate = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, wrapped);
    }
}
