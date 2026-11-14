//! MCP server administration over `_srud/unstable/mcp/*`.
//!
//! These methods edit the file the user may also edit by hand, so what they
//! answer is always what the file says — a change that could not be written is
//! not reported as applied.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use srud_mcp::{Client, HttpEndpoint, ServerCommand, ToolInfo, Transport};
use srud_protocol::acp::AcpError;
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};
use srud_protocol::srud::methods::{
    DeleteMcpServerRequest, DeleteMcpServerResponse, ImportMcpServersRequest,
    ImportMcpServersResponse, ListMcpServersRequest, ListMcpServersResponse, ListMcpToolsRequest,
    ListMcpToolsResponse, McpCatalog, McpServerSpec, McpToolInfo, SetMcpServerRequest,
    SetMcpServerResponse,
};

use super::{parse_params, serialize, Agent};
use crate::mcp_config::{McpError, McpStore};

/// How long a server gets to answer before the question is given up on.
///
/// Connecting is the slow part — `npx` may be fetching a package — and a panel
/// that spins forever is worse than one that says it could not reach the server.
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

impl Agent {
    /// The MCP server configuration, when the host supplied one.
    pub(super) fn mcp_store(&self) -> Result<&Arc<McpStore>, AcpError> {
        self.mcp.as_ref().ok_or_else(|| {
            AcpError::new(
                METHOD_NOT_FOUND,
                "this agent was not given an MCP server configuration",
            )
        })
    }

    pub(super) fn handle_mcp_list(&self, params: Value) -> Result<Value, AcpError> {
        let _: ListMcpServersRequest = parse_params(params)?;
        let store = self.mcp_store()?;
        // Read again rather than answer from memory: the file is the user's to
        // edit, and this is the call a panel makes when it opens.
        Ok(list_reply(store.reload().map_err(mcp_rejected)?))
    }

    pub(super) fn handle_mcp_set(&self, params: Value) -> Result<Value, AcpError> {
        let request: SetMcpServerRequest = parse_params(params)?;
        let store = self.mcp_store()?;
        let servers = store.upsert(request.server).map_err(mcp_rejected)?;
        Ok(serialize(SetMcpServerResponse {
            catalog: McpCatalog { servers },
            meta: None,
        }))
    }

    pub(super) fn handle_mcp_delete(&self, params: Value) -> Result<Value, AcpError> {
        let request: DeleteMcpServerRequest = parse_params(params)?;
        let store = self.mcp_store()?;
        let servers = store.delete(&request.name).map_err(mcp_rejected)?;
        Ok(serialize(DeleteMcpServerResponse {
            catalog: McpCatalog { servers },
            meta: None,
        }))
    }

    /// Stores a whole pasted configuration in one write.
    ///
    /// What it added and what it replaced travel back separately, because the
    /// two mean different things to whoever pasted: a name that was already
    /// there is a server whose command they may have just changed.
    pub(super) fn handle_mcp_import(&self, params: Value) -> Result<Value, AcpError> {
        let request: ImportMcpServersRequest = parse_params(params)?;
        let store = self.mcp_store()?;
        let imported = store.import(request.servers).map_err(mcp_rejected)?;
        Ok(serialize(ImportMcpServersResponse {
            catalog: McpCatalog {
                servers: imported.servers,
            },
            added: imported.added,
            replaced: imported.replaced,
            meta: None,
        }))
    }
    /// Connects to a server and reports what it declares.
    ///
    /// A server that is not saved yet is asked the same question a saved one
    /// is: the connection is made from what the request carries, so the panel
    /// can show what a command gives before anyone stores it.
    ///
    /// The connection is dropped as this returns, which stops a server the agent
    /// started. Two things follow: a probe is not a session, and asking again
    /// asks the server again.
    pub(super) async fn handle_mcp_tools(&self, params: Value) -> Result<Value, AcpError> {
        let request: ListMcpToolsRequest = parse_params(params)?;
        let tools = probe(&request.server).await?;
        Ok(serialize(ListMcpToolsResponse {
            tools: tools
                .into_iter()
                .map(|tool| McpToolInfo {
                    name: tool.name,
                    description: tool.description,
                })
                .collect(),
            meta: None,
        }))
    }
}

/// Connects to the server a configuration describes and lists its tools.
async fn probe(server: &McpServerSpec) -> Result<Vec<ToolInfo>, AcpError> {
    let (name, transport) = transport_of(server);
    let listed = tokio::time::timeout(PROBE_TIMEOUT, async {
        let client = Client::connect(transport).await?;
        client.tool_info().await
    })
    .await;

    match listed {
        Ok(Ok(tools)) => Ok(tools),
        Ok(Err(err)) => Err(AcpError::new(
            INTERNAL_ERROR,
            format!("MCP server `{name}` could not be reached: {err}"),
        )),
        Err(_) => Err(AcpError::new(
            INTERNAL_ERROR,
            format!(
                "MCP server `{name}` did not answer within {} seconds",
                PROBE_TIMEOUT.as_secs()
            ),
        )),
    }
}

fn transport_of(server: &McpServerSpec) -> (String, Transport) {
    match server {
        McpServerSpec::Stdio {
            name,
            command,
            args,
            env,
        } => {
            let mut started = ServerCommand::new(command.clone());
            for arg in args {
                started = started.arg(arg);
            }
            for variable in env {
                started = started.env(&variable.name, &variable.value);
            }
            (name.clone(), started.into())
        }
        McpServerSpec::Http { name, url, headers } => {
            let mut endpoint = HttpEndpoint::new(url);
            for header in headers {
                endpoint = endpoint.header(&header.name, &header.value);
            }
            (name.clone(), endpoint.into())
        }
    }
}

fn list_reply(servers: Vec<McpServerSpec>) -> Value {
    serialize(ListMcpServersResponse {
        catalog: McpCatalog { servers },
        meta: None,
    })
}

/// What a client is told when a change was refused.
///
/// A fault in the request is told apart from a fault in the machine: a name or a
/// URL the caller got wrong is theirs to fix, while a file that cannot be
/// written is not something they can do anything about. The rule is named as
/// well, so a client can word it in its own language instead of printing the
/// agent's sentence.
fn mcp_rejected(err: McpError) -> AcpError {
    let code = match &err {
        McpError::Invalid(_) | McpError::UnknownServer(_) => INVALID_PARAMS,
        McpError::Config(_) | McpError::Io { .. } | McpError::Parse { .. } => INTERNAL_ERROR,
    };
    let error = AcpError::new(code, err.to_string());
    match err.kind() {
        Some(kind) => error.data(serde_json::json!({
            "kind": kind,
            "params": err.params(),
        })),
        None => error,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::agent::support::*;
    use crate::test_env::Guard;
    use srud_protocol::acp::methods::{
        SRUD_MCP_DELETE, SRUD_MCP_IMPORT, SRUD_MCP_LIST, SRUD_MCP_SET, SRUD_MCP_TOOLS,
    };

    /// An agent whose MCP servers live in a file of this test's own.
    fn mcp_agent() -> (Agent, Guard) {
        let env = home();
        let store = Arc::new(McpStore::load().expect("an empty store"));
        (agent(scripted(vec![])).with_mcp_store(store), env)
    }

    /// A stdio server as a client sends it.
    fn stdio(name: &str, command: &str) -> Value {
        serde_json::json!({
            "transport": "stdio",
            "name": name,
            "command": command,
            "args": ["--root"],
            "env": [{ "name": "TOKEN", "value": "t" }],
        })
    }

    #[tokio::test]
    async fn the_servers_round_trip_over_rpc() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;

        let listed = result_of(
            agent
                .handle(request(SRUD_MCP_LIST, serde_json::json!({})))
                .await,
        );
        assert_eq!(listed["servers"], serde_json::json!([]));

        let saved = result_of(
            agent
                .handle(request(
                    SRUD_MCP_SET,
                    serde_json::json!({ "server": stdio("docs", "run") }),
                ))
                .await,
        );
        assert_eq!(
            saved["servers"],
            serde_json::json!([{
                "transport": "stdio",
                "name": "docs",
                "command": "run",
                "args": ["--root"],
                "env": [{ "name": "TOKEN", "value": "t" }],
            }])
        );

        // The same name again is an edit, not a second server.
        let edited = result_of(
            agent
                .handle(request(
                    SRUD_MCP_SET,
                    serde_json::json!({ "server": stdio("docs", "other") }),
                ))
                .await,
        );
        assert_eq!(edited["servers"].as_array().unwrap().len(), 1);
        assert_eq!(edited["servers"][0]["command"], serde_json::json!("other"));

        let removed = result_of(
            agent
                .handle(request(
                    SRUD_MCP_DELETE,
                    serde_json::json!({ "name": "docs" }),
                ))
                .await,
        );
        assert_eq!(removed["servers"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn a_saved_server_lands_in_the_file_the_user_edits() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;
        result_of(
            agent
                .handle(request(
                    SRUD_MCP_SET,
                    serde_json::json!({
                        "server": {
                            "transport": "http",
                            "name": "remote",
                            "url": "https://example.test/mcp",
                        }
                    }),
                ))
                .await,
        );

        let path = crate::config::mcp_path().expect("a configuration directory");
        assert_eq!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some(crate::config::MCP_FILE)
        );
        let text = std::fs::read_to_string(&path).expect("the file was written");
        // The shape a reader would edit, not the shape a client sends.
        assert!(
            text.contains("\"url\": \"https://example.test/mcp\""),
            "{text}"
        );
        assert!(!text.contains("transport"), "{text}");
    }

    #[tokio::test]
    async fn a_file_edited_by_hand_is_what_the_next_list_reports() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;
        let path = crate::config::mcp_path().expect("a configuration directory");
        std::fs::write(
            &path,
            r#"{ "servers": [{ "name": "hand", "command": "run" }] }"#,
        )
        .expect("written by hand");

        let listed = result_of(
            agent
                .handle(request(SRUD_MCP_LIST, serde_json::json!({})))
                .await,
        );

        assert_eq!(
            listed["servers"],
            serde_json::json!([{
                "transport": "stdio",
                "name": "hand",
                "command": "run",
                "args": [],
                "env": [],
            }])
        );
    }

    #[tokio::test]
    async fn a_refused_server_names_the_rule_it_broke() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(
                    SRUD_MCP_SET,
                    serde_json::json!({ "server": stdio("my server", "run") }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        let data = error.data.expect("the rule travels with the refusal");
        assert_eq!(data["kind"], serde_json::json!("bad-name"));
        assert_eq!(data["params"]["name"], serde_json::json!("my server"));
    }

    #[tokio::test]
    async fn removing_a_server_nobody_configured_names_the_rule() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(
                    SRUD_MCP_DELETE,
                    serde_json::json!({ "name": "docs" }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        let data = error.data.expect("the rule travels with the refusal");
        assert_eq!(data["kind"], serde_json::json!("unknown-server"));
    }

    #[tokio::test]
    async fn an_import_reports_what_it_added_and_what_it_replaced() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;
        result_of(
            agent
                .handle(request(
                    SRUD_MCP_SET,
                    serde_json::json!({ "server": stdio("docs", "first") }),
                ))
                .await,
        );

        let imported = result_of(
            agent
                .handle(request(
                    SRUD_MCP_IMPORT,
                    serde_json::json!({
                        "servers": [stdio("docs", "second"), stdio("files", "run")]
                    }),
                ))
                .await,
        );

        assert_eq!(imported["added"], serde_json::json!(["files"]));
        assert_eq!(imported["replaced"], serde_json::json!(["docs"]));
        assert_eq!(imported["servers"].as_array().unwrap().len(), 2);
        assert_eq!(
            imported["servers"][0]["command"],
            serde_json::json!("second")
        );
    }

    #[tokio::test]
    async fn an_import_the_agent_refuses_writes_nothing() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(
                    SRUD_MCP_IMPORT,
                    serde_json::json!({
                        "servers": [stdio("docs", "run"), stdio("my server", "run")]
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        let data = error.data.expect("the rule travels with the refusal");
        assert_eq!(data["kind"], serde_json::json!("bad-name"));
        assert_eq!(data["params"]["name"], serde_json::json!("my server"));
        // Nothing of the batch landed.
        let listed = result_of(
            agent
                .handle(request(SRUD_MCP_LIST, serde_json::json!({})))
                .await,
        );
        assert_eq!(listed["servers"], serde_json::json!([]));
    }

    /// A server the tests can start: the same script the client crate's own
    /// tests use, so the probe meets a server that really speaks MCP.
    fn python_server(name: &str, command: &str) -> Value {
        serde_json::json!({
            "transport": "stdio",
            "name": name,
            "command": command,
            "args": [concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../mcp/tests/fixtures/mock_server.py"
            )],
            "env": [],
        })
    }

    #[tokio::test]
    async fn the_tools_a_server_declares_are_what_it_answers() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;

        let listed = result_of(
            agent
                .handle(request(
                    SRUD_MCP_TOOLS,
                    serde_json::json!({ "server": python_server("docs", "python3") }),
                ))
                .await,
        );

        assert_eq!(
            listed["tools"],
            serde_json::json!([
                { "name": "echo", "description": "Echo the text back" },
                { "name": "add", "description": "Add two numbers" },
            ])
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_start_is_named_in_the_failure() {
        let (agent, _env) = mcp_agent();
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(
                    SRUD_MCP_TOOLS,
                    serde_json::json!({
                        "server": python_server("docs", "/nonexistent/mcp-server")
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INTERNAL_ERROR);
        assert!(
            error.message.contains("`docs`"),
            "the panel can only fix what it can name: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn an_agent_without_a_file_answers_method_not_found() {
        let _env = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;

        let error = error_of(
            agent
                .handle(request(SRUD_MCP_LIST, serde_json::json!({})))
                .await,
        );

        assert_eq!(i32::from(error.code), METHOD_NOT_FOUND);
    }
}
