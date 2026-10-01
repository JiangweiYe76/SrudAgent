//! JSON-RPC error codes used on the wire.
//!
//! ACP's error object is `{code, message, data?}` — note that it has **no**
//! `_meta` field, so SrudAgent's custom error context goes in `data`.
//!
//! # Code allocation
//!
//! SrudAgent's custom codes live in the JSON-RPC implementation-defined
//! range `-32001`..`-32005`, skipping `-32002`. `agent-client-protocol-schema`
//! 1.9.1's `ErrorCode` enum already claims `-32000` (auth required) and
//! `-32002` (resource not found), so the custom range avoids both: returning
//! `-32002` would be parsed by an ACP-aware client as `ResourceNotFound`.
//! Auth-required needs no custom code — reuse the upstream `-32000`.

/// Parse error: the payload was not valid JSON.
pub const PARSE_ERROR: i32 = -32700;
/// Invalid Request: the JSON-RPC envelope was structurally wrong.
pub const INVALID_REQUEST: i32 = -32600;
/// Method not found: unknown method, including `_srud/*` methods this agent
/// does not implement.
pub const METHOD_NOT_FOUND: i32 = -32601;
/// Invalid params: the method exists but its parameters were rejected.
pub const INVALID_PARAMS: i32 = -32602;
/// Internal error: the default termination for an `Error` turn end.
pub const INTERNAL_ERROR: i32 = -32603;

/// Session not found: the `sessionId` does not exist.
pub const SESSION_NOT_FOUND: i32 = -32001;
/// Turn not found: `session/cancel` referenced a turn that is not active
/// (matched via `_meta.srud.turnId`).
///
/// Note: `-32002` is taken by upstream `ResourceNotFound`, hence `-32003`.
pub const TURN_NOT_FOUND: i32 = -32003;
/// Session busy: an operation cannot run while another is active — e.g.
/// `_srud/unstable/session/steer` against a session with no active turn.
pub const SESSION_BUSY: i32 = -32004;
/// Config error: an invalid model/mode/policy combination was requested.
pub const CONFIG_ERROR: i32 = -32005;
/// Auth required: reuses the upstream `ErrorCode::AuthRequired` value, not a
/// new code. The agent declares `authMethods: []`, so it never returns this.
pub const AUTH_REQUIRED: i32 = -32000;

/// The set of SrudAgent custom codes, for collision checks.
pub const SRUD_CUSTOM_CODES: &[i32] = &[
    SESSION_NOT_FOUND,
    TURN_NOT_FOUND,
    SESSION_BUSY,
    CONFIG_ERROR,
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::ErrorCode;

    #[test]
    fn custom_codes_stay_in_the_jsonrpc_implementation_range() {
        for code in SRUD_CUSTOM_CODES {
            assert!(
                (-32099..=-32000).contains(code),
                "{code} outside the implementation-defined range"
            );
        }
    }

    #[test]
    fn custom_codes_do_not_collide_with_upstream_enums() {
        // Every custom code must round-trip to ErrorCode::Other, never to a
        // named upstream variant — otherwise an ACP-aware client would
        // interpret it with the upstream meaning.
        for code in SRUD_CUSTOM_CODES {
            let parsed = ErrorCode::from(*code);
            assert!(
                matches!(parsed, ErrorCode::Other(_)),
                "code {code} parses as {parsed:?}, not Other"
            );
        }
    }

    #[test]
    fn custom_codes_are_unique() {
        let mut sorted = SRUD_CUSTOM_CODES.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SRUD_CUSTOM_CODES.len());
    }
}
