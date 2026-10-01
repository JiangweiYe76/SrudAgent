//! Tauri IPC transport — the path the desktop client uses.
//!
//! The webview and the Rust main process speak JSON-RPC 2.0 through exactly
//! three Tauri commands. This module defines the **payload types** those
//! commands carry; dispatch lives in the server crate.
//!
//! | Command        | Direction       | Payload                         |
//! |----------------|-----------------|---------------------------------|
//! | `rpc_request`  | client → agent  | a JSON-RPC request              |
//! | `rpc_notify`   | agent → client  | a notification *or* reverse request (distinguished by `id`) |
//! | `rpc_respond`  | client → agent  | a JSON-RPC response             |
//!
//! `rpc_notify` carries both `session/update` notifications (no `id`) and
//! agent-initiated requests like `session/request_permission` (with `id`,
//! awaiting `rpc_respond`) — matching JSON-RPC 2.0 itself, no extra fields.
//!
//! # Connection lifecycle
//!
//! ACP is transport-agnostic; a custom transport must preserve the JSON-RPC
//! message format and lifecycle. For Tauri IPC:
//!
//! - **connect** = the webview finishes loading; the first message must be
//!   `initialize`.
//! - **disconnect** = the webview is destroyed or navigates away; any active
//!   turn is cancelled and its `session/prompt` resolves with `cancelled`.

use serde::{Deserialize, Serialize};

use crate::acp::{JsonRpcMessage, RequestId};

/// Command name: client → agent request. The webview `invoke`s this and the
/// returned promise resolves to the JSON-RPC response.
pub const RPC_REQUEST_COMMAND: &str = "rpc_request";
/// Event name: agent → client notification or reverse request. The webview
/// `listen`s to this.
pub const RPC_NOTIFY_EVENT: &str = "rpc_notify";
/// Command name: client → agent response to a reverse request.
pub const RPC_RESPOND_COMMAND: &str = "rpc_respond";

/// Payload of the `rpc_request` command: a JSON-RPC request envelope.
///
/// `JsonRpcMessage<Request<...>>` serialises to
/// `{jsonrpc:"2.0", id, method, params}`.
pub type RpcRequest = JsonRpcMessage<crate::acp::Request<serde_json::Value>>;

/// Payload of the `rpc_notify` event.
///
/// Either a notification (`{jsonrpc, method, params}`, no `id`) or an
/// agent-initiated request (`{jsonrpc, id, method, params}`) that the client
/// must answer via [`RpcRespond`]. Both are `JsonRpcMessage` over an untagged
/// request/notification union, so the `id` presence distinguishes them.
pub type RpcNotify = JsonRpcMessage<NotifyBody>;

/// Payload of the `rpc_respond` command: a JSON-RPC response to a reverse
/// request, keyed by the request's `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcRespond {
    /// The `id` of the request being answered.
    pub id: RequestId,
    /// The result payload, or `None` when answering with an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// The error payload, mutually exclusive with `result`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<crate::acp::AcpError>,
}

impl RpcRespond {
    /// A successful response carrying `result`.
    #[must_use]
    pub fn ok(id: RequestId, result: serde_json::Value) -> Self {
        Self {
            id,
            result: Some(result),
            error: None,
        }
    }

    /// An error response.
    #[must_use]
    pub fn err(id: RequestId, error: crate::acp::AcpError) -> Self {
        Self {
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// The body of an [`RpcNotify`]: a request (has `id`) or a notification
/// (does not). Tagged by the presence of `id`, exactly as JSON-RPC 2.0 does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NotifyBody {
    /// An agent-initiated request the client must answer.
    Request(crate::acp::Request<serde_json::Value>),
    /// A one-way notification.
    Notification(crate::acp::Notification<serde_json::Value>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn notify_without_id_is_a_notification() {
        let value = json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": "s", "update": { "sessionUpdate": "plan", "entries": [] } }
        });
        let msg: RpcNotify = serde_json::from_value(value).unwrap();
        assert!(matches!(msg.inner(), NotifyBody::Notification(_)));
    }

    #[test]
    fn notify_with_id_is_a_request() {
        let value = json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "session/request_permission",
            "params": { "sessionId": "s", "options": [], "toolCall": {} }
        });
        let msg: RpcNotify = serde_json::from_value(value).unwrap();
        assert!(matches!(msg.inner(), NotifyBody::Request(req) if req.id == RequestId::Number(7)));
    }

    #[test]
    fn respond_ok_and_err_are_mutually_exclusive() {
        let ok = RpcRespond::ok(RequestId::Number(1), json!({ "outcome": "selected" }));
        assert!(ok.result.is_some() && ok.error.is_none());
        let err = RpcRespond::err(
            RequestId::Number(1),
            crate::acp::AcpError::new(crate::error::METHOD_NOT_FOUND, "no such method"),
        );
        assert!(err.result.is_none() && err.error.is_some());
    }
}
