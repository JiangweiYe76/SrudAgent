//! Where a server is, and how to reach it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;

use http::{HeaderName, HeaderValue};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;

use crate::error::McpError;

/// How the agent reaches one MCP server.
#[derive(Debug, Clone)]
pub enum Transport {
    /// A server started as a child process, spoken to over stdin and stdout.
    Stdio(ServerCommand),
    /// A server reached over Streamable HTTP.
    Http(HttpEndpoint),
}

impl From<ServerCommand> for Transport {
    fn from(command: ServerCommand) -> Self {
        Self::Stdio(command)
    }
}

impl From<HttpEndpoint> for Transport {
    fn from(endpoint: HttpEndpoint) -> Self {
        Self::Http(endpoint)
    }
}

/// How to start one stdio MCP server.
#[derive(Debug, Clone, Default)]
pub struct ServerCommand {
    /// The executable to spawn.
    pub program: String,
    /// Arguments passed to it, in order.
    pub args: Vec<String>,
    /// Extra environment variables, on top of the inherited ones.
    pub env: Vec<(String, String)>,
    /// Directory the server starts in. `None` inherits the agent's own.
    pub cwd: Option<PathBuf>,
}

impl ServerCommand {
    /// Creates a command that runs `program` with no arguments.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    /// Appends one argument.
    #[must_use]
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Adds one environment variable.
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    /// Sets the directory the server starts in.
    #[must_use]
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub(crate) fn into_process(self) -> tokio::process::Command {
        let mut process = tokio::process::Command::new(&self.program);
        process.args(&self.args);
        for (key, value) in &self.env {
            process.env(key, value);
        }
        if let Some(cwd) = &self.cwd {
            process.current_dir(cwd);
        }
        process
    }
}

/// Where one Streamable HTTP MCP server listens, and what to send with every
/// request.
#[derive(Debug, Clone, Default)]
pub struct HttpEndpoint {
    /// The server's URL, path included.
    pub url: String,
    /// Headers sent with every request, such as an API key.
    pub headers: Vec<(String, String)>,
}

impl HttpEndpoint {
    /// Creates an endpoint at `url` with no extra headers.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
        }
    }

    /// Adds one header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    /// The transport configuration this endpoint describes.
    ///
    /// # Errors
    ///
    /// Returns [`McpError::Endpoint`] for a header name or value that cannot be
    /// sent as one, which is refused here rather than dropped silently.
    pub(crate) fn into_config(self) -> Result<StreamableHttpClientTransportConfig, McpError> {
        let mut headers = HashMap::new();
        for (name, value) in &self.headers {
            let header = HeaderName::from_str(name)
                .map_err(|_| McpError::Endpoint(format!("`{name}` is not an HTTP header name")))?;
            let value = HeaderValue::from_str(value).map_err(|_| {
                McpError::Endpoint(format!("the value of header `{name}` cannot be sent"))
            })?;
            headers.insert(header, value);
        }
        Ok(StreamableHttpClientTransportConfig::with_uri(self.url).custom_headers(headers))
    }
}
