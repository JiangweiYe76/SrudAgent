//! What can go wrong on the way to a server or a tool call.

use rmcp::service::{ClientInitializeError, ServiceError};

/// What can go wrong while connecting to or querying an MCP server.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// The child process could not be spawned.
    #[error("could not start the MCP server: {0}")]
    Spawn(#[source] std::io::Error),
    /// The server's address or one of its headers cannot be used.
    #[error("invalid MCP server endpoint: {0}")]
    Endpoint(String),
    /// The server was reached but the MCP handshake did not complete.
    #[error("MCP handshake failed: {0}")]
    Handshake(#[source] Box<ClientInitializeError>),
    /// A request to the server failed.
    #[error("MCP request failed: {0}")]
    Request(#[source] ServiceError),
    /// The server answered in a way this client cannot act on.
    #[error("unsupported MCP answer: {0}")]
    Unsupported(&'static str),
    /// Closing the connection failed.
    #[error("MCP shutdown failed: {0}")]
    Shutdown(#[source] tokio::task::JoinError),
}
