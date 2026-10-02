//! Request/response types for SrudAgent's `_srud/unstable/*` methods.
//!
//! These are the only protocol types SrudAgent defines itself.
//! They ride on ACP's extension mechanism: `_`-prefixed method names, declared
//! in `initialize` under `agentCapabilities._meta.srud.unstable`. The
//! `unstable` path segment signals they may change without a protocol review.
//!
//! Like all ACP messages they carry a reserved `_meta` field; SrudAgent's own
//! data goes under `_meta.srud` via [`crate::srud::meta::SrudMeta`].

use serde::{Deserialize, Serialize};

use crate::acp::{ContentBlock, Meta, SessionId};

/// Client -> agent: `_srud/unstable/session/fork`.
///
/// Forks a session at its current history point. ACP has no fork concept, and
/// the upstream `session/fork` sits behind an unstable feature; SrudAgent keeps
/// forking in its own namespace so third-party clients simply ignore it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionRequest {
    /// The session to fork.
    pub session_id: SessionId,
    /// Optional title for the forked session; the agent generates one when
    /// absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`ForkSessionRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkSessionResponse {
    /// The id of the newly created session.
    pub session_id: SessionId,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Client -> agent: `_srud/unstable/session/steer`.
///
/// Injects user input into a turn that is already in flight. ACP v1's
/// `session/prompt` is blocking (one turn at a time), so steering is the
/// escape hatch: the input is appended to the active turn's context and the
/// model sees it at the next step boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerRequest {
    /// The session whose active turn receives the input.
    pub session_id: SessionId,
    /// Content blocks to inject. Same vocabulary as `session/prompt`.
    pub prompt: Vec<ContentBlock>,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`SteerRequest`].
///
/// Acknowledges that the input was accepted into the active turn. The turn's
/// eventual `session/prompt` response remains the single termination signal.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerResponse {
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Client -> agent: `_srud/unstable/session/set_title`.
///
/// Renames a session. ACP has no standard equivalent: titles are agent-owned,
/// surfaced to the client through `SessionInfoUpdate`, and there is no
/// client-to-agent method to change one. This lets a user override the title
/// the agent derived from the first message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionTitleRequest {
    /// The session to rename.
    pub session_id: SessionId,
    /// The new title. An empty or whitespace-only string clears the title,
    /// putting the session back to its unnamed state.
    pub title: String,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`SetSessionTitleRequest`].
///
/// Acknowledges the rename. The new title also reaches the client as a
/// `session_info_update`, so a client that watches the update stream learns
/// about renames made anywhere, not just its own.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSessionTitleResponse {
    /// The title now in effect, or `None` when the session is unnamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Client -> agent: `_srud/unstable/session/rollout/read`.
///
/// Reads raw rollout entries for a session. `session/load` replays history as
/// `session/update` notifications, losing the original record structure; this
/// method exposes the stored entries verbatim for diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRolloutRequest {
    /// The session whose rollout to read.
    pub session_id: SessionId,
    /// Zero-based index of the first entry to return. Defaults to the start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Maximum number of entries to return. The agent applies its own cap
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`ReadRolloutRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadRolloutResponse {
    /// The raw rollout entries, in order. Each entry is the persisted JSON
    /// object as written, without protocol-level interpretation.
    pub entries: Vec<serde_json::Value>,
    /// Index of the first entry in the full rollout, so clients can page.
    pub offset: u64,
    /// Total number of entries in the rollout.
    pub total: u64,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fork_request_uses_camel_case_keys() {
        let request = ForkSessionRequest {
            session_id: SessionId::new("sess_01"),
            title: Some("experiment".to_string()),
            meta: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["sessionId"], json!("sess_01"));
        assert_eq!(value["title"], json!("experiment"));
        assert!(value.get("session_id").is_none());
    }

    #[test]
    fn fork_request_round_trips_without_optional_fields() {
        let json_in = json!({ "sessionId": "s" });
        let request: ForkSessionRequest = serde_json::from_value(json_in.clone()).unwrap();
        assert_eq!(request.title, None);
        let json_out = serde_json::to_value(&request).unwrap();
        assert_eq!(json_out, json_in);
    }

    #[test]
    fn steer_request_carries_content_blocks() {
        let request = SteerRequest {
            session_id: SessionId::new("s"),
            prompt: vec![ContentBlock::Text(crate::acp::TextContent::new(
                "stop and summarise",
            ))],
            meta: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["prompt"][0]["type"], json!("text"));
    }

    #[test]
    fn set_title_request_uses_camel_case_keys() {
        let request = SetSessionTitleRequest {
            session_id: SessionId::new("sess_01"),
            title: "Release notes".to_string(),
            meta: None,
        };
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["sessionId"], json!("sess_01"));
        assert_eq!(value["title"], json!("Release notes"));
        assert!(value.get("session_id").is_none());
    }

    #[test]
    fn set_title_response_omits_an_absent_title() {
        let unnamed = serde_json::to_value(SetSessionTitleResponse::default()).unwrap();
        assert!(unnamed.get("title").is_none());
        let named = serde_json::to_value(SetSessionTitleResponse {
            title: Some("Named".to_string()),
            meta: None,
        })
        .unwrap();
        assert_eq!(named, json!({ "title": "Named" }));
    }

    #[test]
    fn rollout_response_shape() {
        let response = ReadRolloutResponse {
            entries: vec![json!({ "type": "turn_started" })],
            offset: 0,
            total: 1,
            meta: None,
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value["total"], json!(1));
        assert_eq!(value["entries"][0]["type"], json!("turn_started"));
    }
}
