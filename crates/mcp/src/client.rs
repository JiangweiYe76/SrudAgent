//! One connection to a server, and the requests made over it.

use rmcp::model::{CallToolRequestParams, CallToolResponse, CallToolResult, JsonObject, Tool};
use rmcp::service::{ClientInitializeError, RoleClient, RunningService};
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;

use crate::error::McpError;
use crate::transport::Transport;

/// One tool a server declares, as a client can describe it without calling it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInfo {
    /// The name the server knows the tool by.
    pub name: String,
    /// What the server says the tool does, empty when it says nothing.
    pub description: String,
}

/// A client connection to one MCP server.
///
/// Dropping it closes the connection; a server this client started itself is
/// stopped with it.
pub struct Client {
    service: RunningService<RoleClient, ()>,
}

impl Client {
    /// Connects to the server and completes the MCP handshake.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Spawn`] when the child process cannot be started,
    /// [`McpError::Endpoint`] when an HTTP endpoint names something that is not
    /// a URL or a header, and [`McpError::Handshake`] when the server is reached
    /// but does not complete `initialize`.
    pub async fn connect(transport: Transport) -> Result<Self, McpError> {
        let service = match transport {
            Transport::Stdio(command) => {
                let transport =
                    TokioChildProcess::new(command.into_process()).map_err(McpError::Spawn)?;
                ().serve(transport).await.map_err(handshake)?
            }
            Transport::Http(endpoint) => {
                let transport = StreamableHttpClientTransport::from_config(endpoint.into_config()?);
                ().serve(transport).await.map_err(handshake)?
            }
        };
        Ok(Self { service })
    }

    /// Lists every tool the server declares, following pagination cursors.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Request`] when the server refuses the request or the
    /// connection has already closed.
    pub async fn list_tools(&self) -> Result<Vec<Tool>, McpError> {
        self.service
            .list_all_tools()
            .await
            .map_err(McpError::Request)
    }

    /// Lists what the server declares, as names and sentences.
    ///
    /// For a caller that shows a server rather than calls it: the schema a tool
    /// declares is not carried here, because nothing this build displays reads
    /// it.
    ///
    /// # Errors
    ///
    /// As [`Client::list_tools`].
    pub async fn tool_info(&self) -> Result<Vec<ToolInfo>, McpError> {
        Ok(self
            .list_tools()
            .await?
            .into_iter()
            .map(|tool| ToolInfo {
                name: tool.name.into_owned(),
                description: tool
                    .description
                    .map(|description| description.into_owned())
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// Runs one of the server's tools.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Request`] when the server refuses the call or the
    /// connection has closed, and [`McpError::Unsupported`] for an answer that
    /// asks the client for something this build cannot supply.
    pub async fn call_tool(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, McpError> {
        let mut params = CallToolRequestParams::new(name.to_owned());
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        match self
            .service
            .call_tool_once(params)
            .await
            .map_err(McpError::Request)?
        {
            CallToolResponse::Complete(result) => Ok(result),
            // A call that stops halfway to ask the client for input, or to hand
            // back a task handle, needs a client that can answer it. Nothing here
            // can, and pretending the call finished would report a result the
            // server never produced.
            CallToolResponse::InputRequired(_) => {
                Err(McpError::Unsupported("the tool asked for client input"))
            }
            CallToolResponse::Task(_) => Err(McpError::Unsupported(
                "the tool was turned into a task the client has to poll",
            )),
            _ => Err(McpError::Unsupported(
                "the server answered the tool call with something unrecognised",
            )),
        }
    }

    /// Identifies the server that answered the handshake.
    #[must_use]
    pub fn server_name(&self) -> Option<String> {
        self.service
            .peer_info()
            .and_then(|info| info.server_info.as_ref().map(|server| server.name.clone()))
    }

    /// How the server asks to be used, when the handshake carried it.
    ///
    /// The text is written for the model rather than for a client UI: when to
    /// reach for this server and when not to. A server that sent nothing, or
    /// only whitespace, reports `None`, so a caller can tell it apart from a
    /// server whose notes are there to be shown.
    #[must_use]
    pub fn instructions(&self) -> Option<String> {
        self.service
            .peer_info()
            .and_then(|info| info.instructions.clone())
            .filter(|text| !text.trim().is_empty())
    }

    /// Closes the connection and stops a server this client started.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Shutdown`] when the transport reports a failure
    /// while closing.
    pub async fn shutdown(self) -> Result<(), McpError> {
        self.service
            .cancel()
            .await
            .map(|_| ())
            .map_err(McpError::Shutdown)
    }
}

/// The handshake failed, kept in one place because either transport can raise it.
fn handshake(err: ClientInitializeError) -> McpError {
    McpError::Handshake(Box::new(err))
}
