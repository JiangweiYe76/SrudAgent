//! Attaching the MCP servers a client names to the session that named them.
//!
//! `session/new`, `session/load` and `session/resume` all carry `mcpServers`.
//! A session answers as soon as it is registered; the servers connect in the
//! background, and the tools they declare are attached when they are ready.
//! The first prompt waits for them for a bounded time, so a slow server cannot
//! hold the conversation.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use srud_core::prompt;
use srud_core::tools::ToolRegistry;
use srud_mcp::{connect_server, HttpEndpoint, ServerCommand, Transport};
use srud_protocol::acp::{AcpError, McpServer, SessionId};
use srud_protocol::error::INVALID_PARAMS;

use super::{session_not_found, Agent};
use crate::sessions::{McpInstructions, McpPhase, McpSession, SessionManager};

/// How long one server gets to start before it is reported as failed.
const START_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a first prompt waits for servers that are still connecting.
///
/// The session itself never waits: this is the point where the model's tool
/// list would otherwise silently lack what the user configured.
const STARTUP_WAIT: Duration = Duration::from_secs(5);

impl Agent {
    /// Starts connecting the servers this session was asked to run with.
    ///
    /// The transports are resolved here, so a transport this agent does not
    /// declare is still a bad request. The connections themselves run after
    /// the session answers; the tools they declare are attached when ready.
    ///
    /// # Errors
    ///
    /// [`INVALID_PARAMS`] for a transport this agent does not declare — the
    /// capabilities never offered it, so the client is out of contract rather
    /// than unlucky — and the session-not-found error when it went away.
    pub(super) fn start_mcp_servers(
        &self,
        id: &SessionId,
        cwd: &Path,
        servers: &[McpServer],
    ) -> Result<(), AcpError> {
        if servers.is_empty() {
            return Ok(());
        }
        let transports = servers
            .iter()
            .map(|server| transport_for(server, cwd))
            .collect::<Result<Vec<_>, _>>()?;
        if !self.sessions.set_mcp_connecting(id) {
            return Err(session_not_found(id));
        }
        let sessions = Arc::clone(&self.sessions);
        let base = Arc::clone(&self.tools);
        let task_id = id.clone();
        let task = tokio::spawn(async move {
            connect_all(sessions, base, task_id, transports).await;
        });
        self.sessions.set_mcp_task(id, task.abort_handle());
        Ok(())
    }

    /// Attaches servers unless a live session already has them.
    ///
    /// A session that is still open keeps the connections it has: the list it
    /// names again is the one it was opened with. A session whose servers
    /// failed is retried.
    pub(super) fn attach_mcp_servers_if_needed(
        &self,
        id: &SessionId,
        cwd: &Path,
        servers: &[McpServer],
    ) -> Result<(), AcpError> {
        match self.sessions.mcp_phase(id) {
            Some(McpPhase::Connecting | McpPhase::Ready) => Ok(()),
            _ => self.start_mcp_servers(id, cwd, servers),
        }
    }

    /// Waits for a session's servers before its first turn reads the tools.
    ///
    /// Bounded: a server that does not answer in time does not hold the
    /// conversation, and its tools are simply absent from this turn.
    pub(super) async fn await_mcp_startup(&self, id: &SessionId) {
        if !self.sessions.take_mcp_wait(id) {
            return;
        }
        let _ = self.sessions.wait_for_mcp(id, STARTUP_WAIT).await;
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

    /// What a session's servers say about themselves, as instruction blocks.
    ///
    /// One `## <server>` section per server that said anything, all under a
    /// single `# MCP servers` heading: each note is a paragraph of its own, so a
    /// flat list would run them together. Empty when no server said anything —
    /// a heading with no server under it is text the model reads for nothing.
    pub(super) fn mcp_sections(&self, id: &SessionId) -> Vec<prompt::Section> {
        let Some(mcp) = self.sessions.mcp(id) else {
            return Vec::new();
        };
        let body = mcp
            .instructions
            .iter()
            .filter(|server| !server.text.trim().is_empty())
            .map(|server| format!("## {}\n\n{}", server.server, server.text.trim()))
            .collect::<Vec<_>>()
            .join("\n\n");
        if body.is_empty() {
            return Vec::new();
        }
        vec![prompt::Section::new("MCP servers", body)]
    }
}

/// Connects one session's servers and attaches what they contribute.
///
/// Servers are connected one after another: each owns a child process or a
/// remote endpoint, and the wait is per server rather than per session. A
/// server that does not answer is left out; the others still attach.
async fn connect_all(
    sessions: Arc<SessionManager>,
    base: Arc<ToolRegistry>,
    id: SessionId,
    servers: Vec<(String, Transport)>,
) {
    let mut tools = Vec::new();
    let mut instructions = Vec::new();
    let mut failures = Vec::new();
    let mut connected = 0;

    for (name, transport) in servers {
        match tokio::time::timeout(START_TIMEOUT, connect_server(&name, transport)).await {
            Ok(Ok(server)) => {
                connected += 1;
                tools.extend(server.tools);
                if let Some(text) = server.instructions {
                    instructions.push(McpInstructions { server: name, text });
                }
            }
            Ok(Err(err)) => failures.push(format!("{name}: {err}")),
            Err(_) => failures.push(format!("{name}: no answer in {}s", START_TIMEOUT.as_secs())),
        }
    }

    if connected == 0 {
        sessions.set_mcp_failed(&id, failures.join("; "));
        return;
    }
    match base.with_tools(tools) {
        Ok(registry) => {
            sessions.set_mcp(
                &id,
                Arc::new(McpSession {
                    tools: Arc::new(registry),
                    instructions,
                    failures,
                }),
            );
        }
        Err(duplicate) => {
            sessions.set_mcp_failed(
                &id,
                format!("two MCP servers declare the same tool: {duplicate}"),
            );
        }
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
    use crate::sessions::McpPhase;
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

    /// The mock server, held back before it answers the first request.
    fn slow_server(seconds: &str) -> Value {
        let mut server = mock_server();
        server["env"] = json!([{ "name": "MCP_STARTUP_DELAY", "value": seconds }]);
        server
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
        assert_eq!(mcp_phase(&agent, &id).await, McpPhase::Ready);

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
    async fn session_new_answers_before_a_slow_server_starts() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;

        let started = std::time::Instant::now();
        let reply = result_of(
            agent
                .handle(request(
                    SESSION_NEW,
                    json!({ "cwd": "", "mcpServers": [slow_server("2")] }),
                ))
                .await,
        );
        let id = SessionId::new(reply["sessionId"].as_str().expect("an id").to_owned());

        assert!(
            started.elapsed() < std::time::Duration::from_millis(1500),
            "session/new waited for the server: {:?}",
            started.elapsed()
        );
        assert_eq!(agent.sessions.mcp_phase(&id), Some(McpPhase::Connecting));

        assert_eq!(mcp_phase(&agent, &id).await, McpPhase::Ready);
        let tools = agent.sessions.tools(&id).expect("the tools arrive later");
        assert!(tools.contains("mcp__docs__echo"));
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
    async fn a_server_that_cannot_start_leaves_the_session_without_it() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;

        let reply = result_of(
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
        let id = SessionId::new(reply["sessionId"].as_str().expect("an id").to_owned());

        assert!(
            matches!(
                mcp_phase(&agent, &id).await,
                McpPhase::Failed(reason) if reason.contains("dead")
            ),
            "the failure names the server"
        );
        assert!(agent.sessions.tools(&id).is_none(), "nothing was attached");
        assert!(
            agent.sessions.get(&id).is_some(),
            "the session still runs without the server's tools"
        );
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        assert!(
            listed["sessions"].as_array().is_some_and(|all| all
                .iter()
                .any(|info| info["sessionId"] == json!(id.0.as_ref()))),
            "the session is still listed"
        );
    }

    #[tokio::test]
    async fn a_server_s_usage_notes_become_one_instruction_block() {
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
        assert_eq!(mcp_phase(&agent, &id).await, McpPhase::Ready);

        let sections = agent.mcp_sections(&id);

        assert_eq!(sections.len(), 1, "one block, however many servers spoke");
        assert_eq!(sections[0].title, "MCP servers");
        assert_eq!(
            sections[0].body, "## docs\n\nUse the echo tool to repeat text.",
            "each server keeps its own heading under the shared one"
        );
    }

    #[tokio::test]
    async fn a_session_with_no_servers_has_no_mcp_block() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let id = new_session(&agent).await;

        assert!(
            agent.mcp_sections(&id).is_empty(),
            "a heading with no server under it is text read for nothing"
        );
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
        assert_eq!(mcp_phase(&agent, &id).await, McpPhase::Ready);

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
        assert_eq!(mcp_phase(&agent, &id).await, McpPhase::Ready);
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
