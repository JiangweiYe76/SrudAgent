//! Request/response types for SrudAgent's `_srud/unstable/*` methods.
//!
//! These are the only protocol types SrudAgent defines itself.
//! They ride on ACP's extension mechanism: `_`-prefixed method names. This
//! build does not advertise them in `agentCapabilities._meta` — a client that
//! does not know them simply never calls them. The `unstable` path segment
//! signals they may change without a protocol review.
//!
//! Like all ACP messages they carry a reserved `_meta` field; SrudAgent's own
//! data goes under `_meta.srud` via [`crate::srud::meta::SrudMeta`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::acp::{ContentBlock, Meta, SessionId, StopReason};

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

/// Client -> agent: `_srud/unstable/session/regenerate`.
///
/// Discards the answer the last turn gave and asks the model again for a fresh
/// one. The message it answered stays in the conversation and is answered once:
/// the client is told to drop the turn it was shown (`TurnDiscarded` →
/// `srud.turnDiscarded`), and the turn that replaces it streams under a new id.
///
/// Resolves when the second turn ends, exactly like `session/prompt`, because a
/// caller streaming the replacement has nothing to wait on otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegenerateRequest {
    /// The session to ask again.
    pub session_id: SessionId,
    /// The turn whose answer is being replaced.
    ///
    /// Named rather than implied: a client that regenerated twice, or a session
    /// two clients are watching, must not have the second request replace a
    /// different turn from the one the user was looking at. A turn that is not
    /// the session's last is refused rather than silently mistaken for it.
    pub turn_id: String,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`RegenerateRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegenerateResponse {
    /// Why the replacement turn stopped.
    pub stop_reason: StopReason,
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

/// One model a provider offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModel {
    /// The id sent to the provider.
    pub id: String,
    /// What the model is called in a menu. Absent means the id is the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the model accepts besides text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<Modality>,
    /// Maximum context window in tokens, as configured by the user. The agent
    /// reports it as `UsageUpdate.size` so clients can render context pressure.
    /// Absent when the model has no configured window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    /// The thinking levels this model accepts, when the user declared them.
    /// Absent means the model has no thinking control and requests carry none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
}

/// The thinking controls a user declared for one model.
///
/// The keys of `levels` are the model-agnostic levels SrudAgent selects
/// between; their values are what this endpoint's wire expects. A level left
/// out is one this model does not support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingConfig {
    /// The level a new session starts on. Must name one of `levels`.
    pub default: ThinkingLevel,
    /// Level to wire value, for the levels this model supports.
    ///
    /// `off` is the exception: a request for it writes the disabled toggle, and
    /// the value recorded here is not sent.
    #[serde(default)]
    pub levels: BTreeMap<ThinkingLevel, String>,
}

/// A thinking level a model is declared with, regardless of its wire values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    /// Thinking off.
    Off,
    /// The least thinking a model can do while still thinking.
    Minimal,
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
    /// Above high, where a model has that tier.
    Xhigh,
    /// The most effort a model offers.
    Max,
}

impl ThinkingLevel {
    /// Every level, weakest to strongest.
    ///
    /// The order is the one used to project a selection onto a model that no
    /// longer offers it: a reader walks this list.
    pub const ALL: [Self; 7] = [
        Self::Off,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    /// The stable spelling of this level.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ThinkingLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ThinkingLevel {
    type Err = String;

    /// Parses the stable spelling, case-insensitively.
    ///
    /// The value comes from a log a user may have hand-edited, so it is matched
    /// rather than trusted: an unknown level is reported, not coerced.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => Err(format!("unknown thinking level `{other}`")),
        }
    }
}

/// Something a model accepts as input, besides the text it always takes.
///
/// `Text` is not a variant: every model takes it, so an entry for it would be
/// noise. A model with an empty modality list is a text-only one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Modality {
    /// Images, as inputs to the prompt.
    Image,
    /// Audio, as inputs to the prompt.
    Audio,
    /// Video, as inputs to the prompt.
    Video,
}

/// One configured provider, as a client is shown it.
///
/// The credential is **not** here whole. `api_key` carries a masked rendering —
/// enough to tell two keys apart, not enough to use one — and `has_key` says
/// whether there is one at all, so a panel can say "set" without ever being told
/// what was set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    /// Stable identifier, used in model references.
    pub id: String,
    /// Human-readable label.
    pub name: String,
    /// Endpoint root, with no trailing slash.
    pub base_url: String,
    /// The credential as it may be displayed, or absent when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Whether a credential is stored.
    pub has_key: bool,
    /// The models this provider offers.
    pub models: Vec<ProviderModel>,
}

/// A provider as a write request describes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    /// Stable identifier, used in model references.
    pub id: String,
    /// Human-readable label.
    pub name: String,
    /// Endpoint root, with no trailing slash.
    pub base_url: String,
    /// The new credential.
    ///
    /// Absent (or `null`) means **keep what is stored**, which is not the same as
    /// clearing it. A panel reads a masked key back and saves the provider
    /// unchanged; sending that mask as the key would replace the real credential
    /// with its own display value. An empty string is how a key is cleared, for
    /// a local endpoint that wants none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// The models this provider offers. May be empty while one is being set
    /// up; validation only refuses blank or duplicated ids.
    #[serde(default)]
    pub models: Vec<ProviderModel>,
}

/// The providers a client is shown, and which model new sessions start from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCatalog {
    /// Every configured provider, in configuration order.
    pub providers: Vec<ProviderView>,
    /// The model reference new sessions start from, when one is chosen. Absent
    /// means no model has been chosen yet.
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "default")]
    pub default_model: Option<String>,
}

/// Client -> agent: `_srud/unstable/provider/list`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProvidersRequest {
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`ListProvidersRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListProvidersResponse {
    /// The providers, and the model new sessions start from.
    #[serde(flatten)]
    pub catalog: ProviderCatalog,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Client -> agent: `_srud/unstable/provider/set`.
///
/// Adds a provider, or replaces the one whose id it names — one method for both
/// because the difference is only whether the id was already there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderRequest {
    /// The provider to store.
    pub provider: ProviderInput,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`SetProviderRequest`].
///
/// Carries the whole catalog rather than an acknowledgement, so a client renders
/// the result of its own edit without asking again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetProviderResponse {
    /// The providers, and the model new sessions start from.
    #[serde(flatten)]
    pub catalog: ProviderCatalog,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Client -> agent: `_srud/unstable/provider/delete`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderRequest {
    /// The provider to remove.
    pub provider_id: String,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`DeleteProviderRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteProviderResponse {
    /// The providers that remain, and the model new sessions start from.
    #[serde(flatten)]
    pub catalog: ProviderCatalog,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Client -> agent: `_srud/unstable/provider/set_default`.
///
/// The only method that changes the global default. Switching a model inside a
/// session goes through `session/set_config_option`, which leaves it alone: a
/// session's choice is its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultProviderRequest {
    /// The model reference new sessions should start from.
    pub model: String,
    /// The `_meta` property reserved by ACP for extensibility.
    #[serde(default, rename = "_meta", skip_serializing_if = "Option::is_none")]
    pub meta: Option<Meta>,
}

/// Response to [`SetDefaultProviderRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultProviderResponse {
    /// The providers, and the model new sessions start from.
    #[serde(flatten)]
    pub catalog: ProviderCatalog,
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

    /// A provider view with a masked key, as `provider/list` reports one.
    fn view() -> ProviderView {
        ProviderView {
            id: "openai".into(),
            name: "OpenAI".into(),
            base_url: "https://api.openai.com/v1".into(),
            api_key: Some("sk-…abcd".into()),
            has_key: true,
            models: vec![ProviderModel {
                id: "gpt-4o".into(),
                name: Some("GPT-4o".into()),
                modalities: vec![Modality::Image],
                context_window: Some(128_000),
                thinking: None,
            }],
        }
    }

    #[test]
    fn a_provider_view_uses_camel_case_keys() {
        let value = serde_json::to_value(view()).unwrap();
        assert_eq!(value["baseUrl"], json!("https://api.openai.com/v1"));
        assert_eq!(value["apiKey"], json!("sk-…abcd"));
        assert_eq!(value["hasKey"], json!(true));
        assert!(value.get("base_url").is_none());
    }

    #[test]
    fn modalities_travel_as_lowercase_names() {
        let value = serde_json::to_value(view()).unwrap();
        assert_eq!(value["models"][0]["modalities"], json!(["image"]));

        // A text-only model says nothing, rather than sending an empty list.
        let mut text_only = view();
        text_only.models[0].modalities = vec![];
        let value = serde_json::to_value(&text_only).unwrap();
        assert!(value["models"][0].get("modalities").is_none());

        let read: ProviderModel = serde_json::from_value(json!({
            "id": "omni",
            "modalities": ["image", "audio", "video"],
        }))
        .unwrap();
        assert_eq!(
            read.modalities,
            vec![Modality::Image, Modality::Audio, Modality::Video]
        );

        // A model written before this field existed reads as text-only.
        let older: ProviderModel = serde_json::from_value(json!({ "id": "plain" })).unwrap();
        assert!(older.modalities.is_empty());
    }

    #[test]
    fn a_thinking_map_round_trips_with_level_keys() {
        let model: ProviderModel = serde_json::from_value(json!({
            "id": "m",
            "thinking": {
                "default": "high",
                "levels": { "low": "low", "xhigh": "high" }
            }
        }))
        .unwrap();
        let thinking = model.thinking.clone().expect("the map was declared");
        assert_eq!(thinking.default, ThinkingLevel::High);
        assert_eq!(
            thinking.levels.get(&ThinkingLevel::Low),
            Some(&"low".to_owned())
        );

        let value = serde_json::to_value(&model).unwrap();
        assert_eq!(value["thinking"]["levels"]["xhigh"], json!("high"));
    }

    #[test]
    fn a_recorded_wire_shape_is_ignored() {
        // An unrecognized key in a declaration is ignored rather than making the
        // record unreadable: the levels survive, and the key is not written back.
        let model: ProviderModel = serde_json::from_value(json!({
            "id": "m",
            "thinking": {
                "default": "high",
                "levels": { "high": "high" },
                "wire": "bailian"
            }
        }))
        .expect("a retired field does not make the record unreadable");

        assert_eq!(
            model.thinking.as_ref().map(|thinking| thinking.default),
            Some(ThinkingLevel::High)
        );
        let value = serde_json::to_value(&model).unwrap();
        assert!(
            value["thinking"].get("wire").is_none(),
            "the retired field is not written back"
        );
    }

    #[test]
    fn an_unknown_thinking_level_is_rejected() {
        let error = serde_json::from_value::<ProviderModel>(json!({
            "id": "m",
            "thinking": { "default": "extreme", "levels": { "high": "high" } }
        }));
        assert!(
            error.is_err(),
            "a level this build cannot select is refused"
        );
    }

    #[test]
    fn a_level_parses_from_its_wire_spelling() {
        assert_eq!("xhigh".parse::<ThinkingLevel>(), Ok(ThinkingLevel::Xhigh));
        assert_eq!(ThinkingLevel::Max.to_string(), "max");
        assert!("extreme".parse::<ThinkingLevel>().is_err());
    }

    #[test]
    fn the_catalog_puts_providers_beside_the_default() {
        let response = ListProvidersResponse {
            catalog: ProviderCatalog {
                providers: vec![view()],
                default_model: Some("openai/gpt-4o".into()),
            },
            meta: None,
        };
        let value = serde_json::to_value(&response).unwrap();
        // Flattened: the catalog's fields sit directly on the response, which is
        // the shape a client reads without unwrapping anything.
        assert_eq!(value["default"], json!("openai/gpt-4o"));
        assert_eq!(value["providers"][0]["id"], json!("openai"));
        assert!(value.get("catalog").is_none());
    }

    #[test]
    fn an_absent_default_is_omitted() {
        let response = ListProvidersResponse {
            catalog: ProviderCatalog {
                providers: vec![],
                default_model: None,
            },
            meta: None,
        };
        let value = serde_json::to_value(&response).unwrap();
        assert_eq!(value, json!({ "providers": [] }));
    }

    #[test]
    fn a_provider_input_without_a_key_keeps_the_stored_one() {
        // The whole point of the field being optional: absent is "do not touch",
        // not "clear".
        let request: SetProviderRequest = serde_json::from_value(json!({ "provider": {
            "id": "openai",
            "name": "OpenAI",
            "baseUrl": "https://api.openai.com/v1",
            "models": [{ "id": "gpt-4o" }],
        }}))
        .unwrap();
        assert_eq!(request.provider.api_key, None);

        let value = serde_json::to_value(&request).unwrap();
        assert!(value["provider"].get("apiKey").is_none());
    }

    #[test]
    fn an_empty_key_is_distinct_from_an_absent_one() {
        let request: SetProviderRequest = serde_json::from_value(json!({ "provider": {
            "id": "local",
            "name": "Local",
            "baseUrl": "http://localhost:8080/v1",
            "apiKey": "",
            "models": [{ "id": "llama" }],
        }}))
        .unwrap();
        assert_eq!(request.provider.api_key.as_deref(), Some(""));
    }

    #[test]
    fn a_provider_model_may_omit_its_name() {
        let model: ProviderModel = serde_json::from_value(json!({ "id": "gpt-4o" })).unwrap();
        assert_eq!(model.name, None);
        assert_eq!(
            serde_json::to_value(&model).unwrap(),
            json!({ "id": "gpt-4o" }),
            "an unnamed model does not carry a null name"
        );
    }

    #[test]
    fn a_delete_request_names_the_provider() {
        let request: DeleteProviderRequest =
            serde_json::from_value(json!({ "providerId": "openai" })).unwrap();
        assert_eq!(request.provider_id, "openai");
    }

    #[test]
    fn a_default_request_names_a_model_reference() {
        let request: SetDefaultProviderRequest =
            serde_json::from_value(json!({ "model": "openai/gpt-4o" })).unwrap();
        assert_eq!(request.model, "openai/gpt-4o");
    }

    #[test]
    fn the_mutation_responses_carry_the_whole_catalog() {
        let catalog = ProviderCatalog {
            providers: vec![view()],
            default_model: None,
        };
        for value in [
            serde_json::to_value(SetProviderResponse {
                catalog: catalog.clone(),
                meta: None,
            })
            .unwrap(),
            serde_json::to_value(DeleteProviderResponse {
                catalog: catalog.clone(),
                meta: None,
            })
            .unwrap(),
            serde_json::to_value(SetDefaultProviderResponse {
                catalog: catalog.clone(),
                meta: None,
            })
            .unwrap(),
        ] {
            assert_eq!(value["providers"][0]["id"], json!("openai"));
        }
    }
}
