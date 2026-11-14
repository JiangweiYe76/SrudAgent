//! Attaching the MCP servers a client names to the session that named them.
//!
//! `session/new`, `session/load` and `session/resume` all carry `mcpServers`.
//! The servers are connected before the call answers, so a client that is told a
//! session is ready can rely on the tools it asked for being there.

use std::path::Path;
use std::sync::Arc;

use srud_core::tools::ToolRegistry;
use srud_mcp::{connect_tools, HttpEndpoint, ServerCommand, Transport};
use srud_protocol::acp::{AcpError, McpServer, SessionId};
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS};

use super::{session_not_found, Agent};

impl Agent {
    /// Connects the servers this session was asked to run with.
    ///
    /// The session's tools become the agent's own set plus what the servers
    /// declare, named `mcp__<server>__<tool>`. A session that named no servers
    /// keeps the agent's set untouched.
    ///
    /// # Errors
    ///
    /// [`INVALID_PARAMS`] for a transport this agent does not declare — the
    /// capabilities never offered it, so the client is out of contract rather
    /// than unlucky — and [`INTERNAL_ERROR`] for a server that could not be
    /// reached or whose tools collide with one already named.
    pub(super) async fn attach_mcp_servers(
        &self,
        id: &SessionId,
        cwd: &Path,
        servers: &[McpServer],
    ) -> Result<(), AcpError> {
        if servers.is_empty() {
            return Ok(());
        }
        let mut tools = Vec::new();
        for server in servers {
            let (name, transport) = transport_for(server, cwd)?;
            let connected = connect_tools(&name, transport).await.map_err(|err| {
                AcpError::new(
                    INTERNAL_ERROR,
                    format!("MCP server `{name}` could not be reached: {err}"),
                )
            })?;
            tools.extend(connected);
        }
        let registry = self.tools.with_tools(tools).map_err(|duplicate| {
            AcpError::new(
                INTERNAL_ERROR,
                format!("two MCP servers declare the same tool: {duplicate}"),
            )
        })?;
        if !self.sessions.set_tools(id, Arc::new(registry)) {
            return Err(session_not_found(id));
        }
        Ok(())
    }

    /// The tools a session runs with.
    ///
    /// The set its MCP servers contributed, or the agent's own when it attached
    /// none. Resolved per call so a turn that outlives a `session/load` keeps
    /// running on the set it started with.
    pub(super) fn tools_for(&self, id: &SessionId) -> Arc<ToolRegistry> {
        self.sessions
            .tools(id)
            .unwrap_or_else(|| Arc::clone(&self.tools))
    }
}

/// A server's name beside the way to reach it.
///
/// The two travel together because a failure has to name the server it came
/// from, and the name is only reachable by matching the variant that also
/// yields the transport.
fn transport_for(server: &McpServer, cwd: &Path) -> Result<(String, Transport), AcpError> {
    match server {
        McpServer::Stdio(stdio) => {
            let mut command = ServerCommand::new(stdio.command.to_string_lossy().into_owned());
            for arg in &stdio.args {
                command = command.arg(arg);
            }
            for env in &stdio.env {
                command = command.env(&env.name, &env.value);
            }
            // A session works in its own directory, so a server that resolves
            // relative paths starts where the conversation does rather than in
            // whatever directory the app was launched from.
            Ok((stdio.name.clone(), command.cwd(cwd).into()))
        }
        McpServer::Http(http) => {
            let mut endpoint = HttpEndpoint::new(&http.url);
            for header in &http.headers {
                endpoint = endpoint.header(&header.name, &header.value);
            }
            Ok((http.name.clone(), endpoint.into()))
        }
        // `McpServer` is non-exhaustive, and SSE and ACP are the two transports
        // this agent leaves undeclared.
        _ => Err(AcpError::new(
            INVALID_PARAMS,
            "this agent serves stdio and HTTP MCP servers: its capabilities declare no SSE or ACP transport",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::support::*;
    use futures::stream;
    use serde_json::{json, Value};
    use srud_core::client::{ModelClient, ModelError, ModelEvent, ModelRequest, ModelStream};
    use srud_core::types::ResponseItem;
    use srud_protocol::acp::methods::{
        SESSION_CLOSE, SESSION_LIST, SESSION_LOAD, SESSION_NEW, SESSION_PROMPT,
    };
    use tokio_util::sync::CancellationToken;

    /// A client that answers one round at a time, so a turn can call a tool and
    /// then close.
    struct Rounds {
        rounds: std::sync::Mutex<Vec<Vec<ModelEvent>>>,
    }

    #[async_trait::async_trait]
    impl ModelClient for Rounds {
        async fn stream(
            &self,
            _request: ModelRequest,
            _cancel: CancellationToken,
        ) -> Result<ModelStream, ModelError> {
            let mut rounds = self.rounds.lock().expect("rounds lock");
            if rounds.is_empty() {
                return Err(ModelError::Rejected("script exhausted".into()));
            }
            let events = rounds.remove(0);
            Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
        }
    }

    /// A server the tests can start for real: the same script the client
    /// crate's own tests use, so both layers are exercised against it.
    fn mock_server() -> Value {
        json!({
            "name": "docs",
            "command": "python3",
            "args": [concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../mcp/tests/fixtures/mock_server.py"
            )],
            "env": [],
        })
    }

    #[tokio::test]
    async fn a_stdio_server_puts_its_tools_on_the_session() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;

        let reply = result_of(
            agent
                .handle(request(
                    SESSION_NEW,
                    json!({ "cwd": "", "mcpServers": [mock_server()] }),
                ))
                .await,
        );
        let id = SessionId::new(reply["sessionId"].as_str().expect("an id").to_owned());

        let tools = agent
            .sessions
            .tools(&id)
            .expect("the session runs with what its server declared");
        assert!(tools.contains("mcp__docs__echo"));
        assert!(tools.contains("mcp__docs__add"));
        assert!(
            !agent.tools.contains("mcp__docs__echo"),
            "the tools belong to the session that asked for them"
        );
    }

    #[tokio::test]
    async fn an_sse_server_is_refused_and_leaves_no_session() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(
                    SESSION_NEW,
                    json!({
                        "cwd": "",
                        "mcpServers": [{
                            "type": "sse",
                            "name": "docs",
                            "url": "https://example.com/sse",
                            "headers": [],
                        }],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        assert_eq!(
            listed["sessions"],
            json!([]),
            "a session nobody could connect servers for is not left behind"
        );
    }

    #[test]
    fn an_http_server_becomes_an_endpoint_with_its_headers() {
        let server: McpServer = serde_json::from_value(json!({
            "type": "http",
            "name": "docs",
            "url": "https://example.com/mcp",
            "headers": [{ "name": "Authorization", "value": "Bearer token" }],
        }))
        .expect("an HTTP server config");

        let (name, transport) =
            transport_for(&server, Path::new("/workspace")).expect("HTTP is declared");

        assert_eq!(name, "docs");
        let Transport::Http(endpoint) = transport else {
            panic!("an HTTP server is reached over HTTP");
        };
        assert_eq!(endpoint.url, "https://example.com/mcp");
        assert_eq!(
            endpoint.headers,
            [("Authorization".to_owned(), "Bearer token".to_owned())]
        );
    }

    #[test]
    fn a_stdio_server_starts_in_the_session_workspace() {
        let server: McpServer = serde_json::from_value(json!({
            "name": "docs",
            "command": "/usr/bin/mcp-server",
            "args": ["--root", "."],
            "env": [{ "name": "TOKEN", "value": "abc" }],
        }))
        .expect("a stdio server config");

        let (name, transport) =
            transport_for(&server, Path::new("/workspace")).expect("stdio is the baseline");

        assert_eq!(name, "docs");
        let Transport::Stdio(command) = transport else {
            panic!("a stdio server is started, not dialled");
        };
        assert_eq!(command.program, "/usr/bin/mcp-server");
        assert_eq!(command.args, ["--root", "."]);
        assert_eq!(command.env, [("TOKEN".to_owned(), "abc".to_owned())]);
        assert_eq!(command.cwd.as_deref(), Some(Path::new("/workspace")));
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_fails_the_session() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(
                    SESSION_NEW,
                    json!({
                        "cwd": "",
                        "mcpServers": [{
                            "name": "dead",
                            "command": "/nonexistent/mcp-server",
                            "args": [],
                            "env": [],
                        }],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INTERNAL_ERROR);
        assert!(
            error.message.contains("dead"),
            "the failure names the server: {}",
            error.message
        );
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        assert_eq!(listed["sessions"], json!([]));
    }

    #[tokio::test]
    async fn a_session_with_no_servers_runs_on_the_agent_set() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let id = new_session(&agent).await;

        assert!(agent.sessions.tools(&id).is_none());
        assert!(
            Arc::ptr_eq(&agent.tools_for(&id), &agent.tools),
            "a session with no servers runs on the set the agent was built with"
        );
    }

    #[tokio::test]
    async fn loading_a_session_reconnects_the_servers_it_names() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let cwd = std::env::temp_dir().join(format!("srud-mcp-load-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("a directory to work in");
        let cwd = cwd.to_string_lossy().into_owned();

        let reply = result_of(
            agent
                .handle(request(
                    SESSION_NEW,
                    json!({ "cwd": cwd, "mcpServers": [mock_server()] }),
                ))
                .await,
        );
        let id = SessionId::new(reply["sessionId"].as_str().expect("an id").to_owned());

        // Closing drops the connections: the session is over, and a server it
        // started stops running.
        result_of(
            agent
                .handle(request(
                    SESSION_CLOSE,
                    json!({ "sessionId": id.0.as_ref() }),
                ))
                .await,
        );
        assert!(
            agent.sessions.tools(&id).is_none(),
            "the tools go with the session that had them"
        );

        // A log holds no record of what was spawned, which is why ACP has the
        // client name the servers again.
        result_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({ "sessionId": id.0.as_ref(), "cwd": cwd, "mcpServers": [mock_server()] }),
                ))
                .await,
        );
        let tools = agent
            .sessions
            .tools(&id)
            .expect("loading reconnects what the client named");
        assert!(tools.contains("mcp__docs__echo"));
    }

    #[tokio::test]
    async fn a_turn_runs_the_tool_a_server_declared() {
        let _home = home();
        let model: Arc<dyn ModelClient> = Arc::new(Rounds {
            rounds: std::sync::Mutex::new(vec![
                vec![
                    ModelEvent::ToolCall {
                        call_id: "call-1".into(),
                        name: "mcp__docs__echo".into(),
                        arguments: json!({ "text": "hi" }).to_string(),
                    },
                    ModelEvent::Done,
                ],
                vec![
                    ModelEvent::TextDelta {
                        delta: "all done".into(),
                    },
                    ModelEvent::Done,
                ],
            ]),
        });
        let agent = crate::Agent::new(model, Arc::new(ToolRegistry::new()));
        initialize(&agent).await;

        let reply = result_of(
            agent
                .handle(request(
                    SESSION_NEW,
                    json!({ "cwd": "", "mcpServers": [mock_server()] }),
                ))
                .await,
        );
        let id = SessionId::new(reply["sessionId"].as_str().expect("an id").to_owned());

        let ended = result_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "echo hi" }],
                    }),
                ))
                .await,
        );
        assert_eq!(ended["stopReason"], json!("end_turn"));

        let session = agent.sessions.get(&id).expect("the session is open");
        let state = session.state();
        let ran = state.history().iter().any(|item| {
            matches!(
                item,
                ResponseItem::FunctionCallOutput { output, .. } if output == "Echo: hi"
            )
        });
        assert!(
            ran,
            "the server's answer reaches the model: {:?}",
            state.history()
        );
    }
}
