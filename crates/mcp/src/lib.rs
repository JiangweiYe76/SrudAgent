//! MCP client: connects stdio and Streamable HTTP servers and exposes the tools
//! they declare.
//!
//! # Layout
//!
//! - `transport` — where a server is and how to reach it.
//! - `client` — one connection, and the requests made over it.
//! - `tools` — what a server declares, as the tools the agent's registry takes.
//! - `error` — what can go wrong on the way to either.

mod client;
mod error;
mod tools;
mod transport;

pub use client::{Client, ToolInfo};
pub use error::McpError;
pub use tools::{connect_tools, TOOL_PREFIX};
pub use transport::{HttpEndpoint, ServerCommand, Transport};
