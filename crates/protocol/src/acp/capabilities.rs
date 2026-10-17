//! The capability shapes SrudAgent declares in `initialize`.
//!
//! Capability negotiation is mandatory in ACP: undeclared means unsupported,
//! and callers must check before invoking an optional method. These
//! constructors are the single source of truth shared by the server and tests:
//!
//! - `fs/*` and `terminal/*` are **false**: the agent and client are
//!   same-machine, so the agent reads disk and executes locally instead of
//!   reverse-calling the editor.
//! - Prompt input is **text only**: `image`, `audio`, and `embeddedContext`
//!   are off because the runtime's turn input is a plain string.
//! - Session lifecycle: `close` and `list` are on, and so are `loadSession`
//!   and `resume`, now that a session's log can rebuild it. `loadSession`
//!   replays the conversation; `resume` restores it without replaying.
//! - The `_srud/unstable/*` extensions are **not advertised**: the runtime
//!   does not implement fork, steer, or log reads.

use crate::acp::{
    AgentCapabilities, BooleanConfigOptionCapabilities, ClientCapabilities,
    ClientSessionCapabilities, FileSystemCapabilities, McpCapabilities, PromptCapabilities,
    SessionCapabilities, SessionCloseCapabilities, SessionListCapabilities,
    SessionResumeCapabilities,
};

/// The `_srud/unstable/session/*` and `_srud/unstable/provider/*` extension
/// method names. They are not advertised in `agentCapabilities._meta`; a client
/// that does not know them simply never calls them.
pub const SRUD_UNSTABLE_METHODS: &[&str] = &[
    "sessionFork",
    "sessionSteer",
    "rolloutRead",
    "sessionSetTitle",
    "providerList",
    "providerSet",
    "providerDelete",
    "providerSetDefault",
];

/// SrudAgent's agent-side capabilities.
#[must_use]
pub fn agent_capabilities() -> AgentCapabilities {
    AgentCapabilities::new()
        .load_session(true)
        .prompt_capabilities(
            PromptCapabilities::new()
                .image(false)
                .audio(false)
                .embedded_context(false),
        )
        .mcp_capabilities(McpCapabilities::new())
        .session_capabilities(
            SessionCapabilities::new()
                .list(SessionListCapabilities::new())
                .close(SessionCloseCapabilities::new())
                .resume(SessionResumeCapabilities::new()),
        )
}

/// SrudAgent's client-side capabilities (what the desktop/CLI client offers).
///
/// `fs` and `terminal` are intentionally false; the client accepts boolean
/// config options (e.g. `brave_mode`).
#[must_use]
pub fn client_capabilities() -> ClientCapabilities {
    ClientCapabilities::new()
        .fs(FileSystemCapabilities::new()
            .read_text_file(false)
            .write_text_file(false))
        .terminal(false)
        .session(
            ClientSessionCapabilities::new().config_options(
                crate::acp::SessionConfigOptionsCapabilities::new()
                    .boolean(BooleanConfigOptionCapabilities::new()),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn agent_capabilities_match_the_contract() {
        let value = serde_json::to_value(agent_capabilities()).unwrap();
        assert_eq!(value["loadSession"], json!(true));
        assert_eq!(value["promptCapabilities"]["image"], json!(false));
        assert_eq!(value["promptCapabilities"]["audio"], json!(false));
        assert_eq!(value["promptCapabilities"]["embeddedContext"], json!(false));
        assert!(value["sessionCapabilities"]["list"].is_object());
        assert!(value["sessionCapabilities"]["close"].is_object());
        assert!(value["sessionCapabilities"]["resume"].is_object());
        assert!(value["sessionCapabilities"]
            .get("additionalDirectories")
            .is_none());
        // Unimplemented extensions must not be advertised.
        assert!(value.get("_meta").is_none());
    }

    #[test]
    fn client_capabilities_disable_fs_and_terminal() {
        let value = serde_json::to_value(client_capabilities()).unwrap();
        assert_eq!(value["fs"]["readTextFile"], json!(false));
        assert_eq!(value["fs"]["writeTextFile"], json!(false));
        assert_eq!(value["terminal"], json!(false));
        assert!(value["session"]["configOptions"]["boolean"].is_object());
    }

    #[test]
    fn unstable_method_names_cover_the_extensions_this_build_defines() {
        assert_eq!(SRUD_UNSTABLE_METHODS.len(), 8);
        assert!(SRUD_UNSTABLE_METHODS.contains(&"sessionFork"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"sessionSteer"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"rolloutRead"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"sessionSetTitle"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"providerList"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"providerSet"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"providerDelete"));
        assert!(SRUD_UNSTABLE_METHODS.contains(&"providerSetDefault"));
    }
}
