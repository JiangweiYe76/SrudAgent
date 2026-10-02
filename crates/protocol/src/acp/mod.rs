//! The standard ACP v1 wire types, re-exported from the upstream schema crate.
//!
//! These types are **not** mirrored here — they come straight from
//! `agent-client-protocol-schema`, so the contract cannot drift from the
//! specification. This module curates the subset SrudAgent's protocol surface
//! actually uses and gives it a stable local path: depend on
//! `srud_protocol::acp`, never on the upstream crate directly.
//!
//! Upstream types are `#[non_exhaustive]` with builder-style constructors
//! (e.g. [`PromptRequest::new`], [`ToolCall::new`]). Standard types carry a
//! reserved `meta: Option<Meta>` field that serialises to `_meta` — SrudAgent's
//! private data hangs off it via [`crate::srud::meta::SrudMeta`].

use agent_client_protocol_schema as acp_schema;
pub use agent_client_protocol_schema::rpc;

pub mod capabilities;
pub mod methods;

// The wire protocol version lives at the crate root, not under `v1`.
pub use acp_schema::ProtocolVersion;

// Method-name constants for the standard methods SrudAgent serves or calls.
pub use acp_schema::v1::{AGENT_METHOD_NAMES, CLIENT_METHOD_NAMES, PROTOCOL_LEVEL_METHOD_NAMES};

// Conversion trait used by the upstream builder setters.
pub use acp_schema::IntoOption;

// JSON-RPC envelopes (also re-exported flat by the upstream `v1` module).
pub use acp_schema::v1::{
    JsonRpcBatch, JsonRpcMessage, Notification, Request, RequestId, Response,
};

// Core session lifecycle: requests and responses.
//
// `session/fork` exists upstream but sits behind the `unstable_session_fork`
// feature; SrudAgent's fork is a `_srud/unstable/*` extension instead
// (see `crate::srud::methods`).
pub use acp_schema::v1::{
    CancelNotification, CloseSessionRequest, CloseSessionResponse, DeleteSessionRequest,
    DeleteSessionResponse, ListSessionsResponse, LoadSessionRequest, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, ResumeSessionRequest, SessionId,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse,
};

// Initialisation and capability negotiation.
pub use acp_schema::v1::{
    AgentAuthCapabilities, AgentCapabilities, AuthMethod, AuthMethodId,
    BooleanConfigOptionCapabilities, ClientCapabilities, ClientSessionCapabilities,
    FileSystemCapabilities, Implementation, InitializeRequest, InitializeResponse, McpCapabilities,
    PromptCapabilities, SessionAdditionalDirectoriesCapabilities, SessionCapabilities,
    SessionCloseCapabilities, SessionConfigOptionsCapabilities, SessionDeleteCapabilities,
    SessionListCapabilities, SessionResumeCapabilities,
};

// Prompt content.
pub use acp_schema::v1::{
    Annotations, AudioContent, BlobResourceContents, ContentBlock, EmbeddedResource,
    EmbeddedResourceResource, ImageContent, ResourceLink, Role, TextContent, TextResourceContents,
};

// Tool calls.
pub use acp_schema::v1::{
    Content as ToolCallContentBlock, Diff as ToolCallDiff, Terminal as ToolCallTerminal, ToolCall,
    ToolCallContent, ToolCallId, ToolCallLocation, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind,
};

// The `session/update` stream — the single progress channel.
pub use acp_schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, ConfigOptionUpdate,
    ContentChunk, Cost, CurrentModeUpdate, MessageId, SessionInfo, SessionInfoUpdate,
    SessionNotification, SessionUpdate, UnstructuredCommandInput, UsageUpdate,
};

// Permission flow (agent -> client reverse request).
pub use acp_schema::v1::{
    PermissionOption, PermissionOptionId, PermissionOptionKind, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SelectedPermissionOutcome,
};

// Plans, configuration options, usage, and stop reasons.
//
// Per-turn token `Usage` sits behind `unstable_end_turn_token_usage`; context
// usage flows through the stable `UsageUpdate` session update instead.
pub use acp_schema::v1::{Plan, PlanEntry, PlanEntryPriority, PlanEntryStatus, StopReason};
pub use acp_schema::v1::{
    SessionConfigBoolean, SessionConfigId, SessionConfigKind, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigOptionValue, SessionConfigSelect,
    SessionConfigSelectGroup, SessionConfigSelectOption, SessionConfigSelectOptions,
    SessionConfigValueId,
};

// Extensibility primitives: `_meta` and arbitrary extension messages.
pub use acp_schema::v1::{ExtNotification, ExtRequest, ExtResponse, Meta};

// `MaybeUndefined` distinguishes "field absent" from "field explicitly null",
// which `SessionInfoUpdate` needs in order to clear a title without erasing the
// other fields of a partial update.
pub use acp_schema::MaybeUndefined;

// JSON-RPC error object and codes.
pub use acp_schema::v1::{Error as AcpError, ErrorCode};
