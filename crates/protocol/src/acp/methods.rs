//! Wire method names, as string constants.
//!
//! The upstream crate exposes standard names through the
//! `*_METHOD_NAMES` structs (re-exported in [`crate::acp`]); this module adds
//! SrudAgent's own `_srud/unstable/*` names alongside them in one flat table,
//! so dispatch code can match on `&'static str` without importing the structs.

/// Client -> agent: capability negotiation. Always the first request.
pub const INITIALIZE: &str = "initialize";
/// Client -> agent: create a session.
pub const SESSION_NEW: &str = "session/new";
/// Client -> agent: send user input; the response marks the end of a turn.
pub const SESSION_PROMPT: &str = "session/prompt";
/// Client -> agent: replay a session's history as `session/update` notifications.
pub const SESSION_LOAD: &str = "session/load";
/// Client -> agent: resume a session without replaying history.
pub const SESSION_RESUME: &str = "session/resume";
/// Client -> agent: list known sessions.
pub const SESSION_LIST: &str = "session/list";
/// Client -> agent: cancel in-flight work and release the session's resources.
pub const SESSION_CLOSE: &str = "session/close";
/// Client -> agent: delete a session.
pub const SESSION_DELETE: &str = "session/delete";
/// Client -> agent: switch the session mode (deprecated in favour of config options).
pub const SESSION_SET_MODE: &str = "session/set_mode";
/// Client -> agent: change a session config option.
pub const SESSION_SET_CONFIG_OPTION: &str = "session/set_config_option";
/// Client -> agent (notification): cancel the active turn; the prompt response
/// then carries `stopReason: "cancelled"`.
pub const SESSION_CANCEL: &str = "session/cancel";

/// Agent -> client: the single progress channel.
pub const SESSION_UPDATE: &str = "session/update";
/// Agent -> client: ask the user to approve an action (approval flow).
pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";

// SrudAgent extensions (`_srud/unstable/*`). Declared in `initialize` via
// `agentCapabilities._meta.srud.unstable`; unknown extension *methods* answer
// with `-32601`, unknown extension *notifications* are ignored.

/// Client -> agent: fork a session at the current history point.
pub const SRUD_SESSION_FORK: &str = "_srud/unstable/session/fork";
/// Client -> agent: inject input into an in-flight turn.
pub const SRUD_SESSION_STEER: &str = "_srud/unstable/session/steer";
/// Client -> agent: read raw rollout entries for a session.
pub const SRUD_SESSION_ROLLOUT_READ: &str = "_srud/unstable/session/rollout/read";
