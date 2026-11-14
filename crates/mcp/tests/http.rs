//! Connects to a Streamable HTTP server over a real socket.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use srud_mcp::{Client, HttpEndpoint, McpError, Transport};

/// A mock server on an ephemeral port, stopped when the test ends.
struct MockServer {
    child: Child,
    url: String,
}

impl MockServer {
    fn start() -> Self {
        let mut child = Command::new("python3")
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/mock_http_server.py"
            ))
            .stdout(Stdio::piped())
            .spawn()
            .expect("the mock server starts");
        let mut port = String::new();
        BufReader::new(child.stdout.take().expect("a pipe"))
            .read_line(&mut port)
            .expect("the server prints its port");
        Self {
            child,
            url: format!("http://127.0.0.1:{}/mcp", port.trim()),
        }
    }

    fn endpoint(&self) -> HttpEndpoint {
        HttpEndpoint::new(&self.url).header("Authorization", "Bearer test-token")
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test]
async fn connects_and_lists_tools() {
    let server = MockServer::start();
    let client = Client::connect(Transport::Http(server.endpoint()))
        .await
        .expect("connect");

    assert_eq!(client.server_name().as_deref(), Some("mock"));
    let tools = client.list_tools().await.expect("list tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(names, ["echo", "add"]);

    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn calls_a_tool() {
    let server = MockServer::start();
    let client = Client::connect(Transport::Http(server.endpoint()))
        .await
        .expect("connect");

    let arguments = serde_json::json!({ "text": "hello" });
    let result = client
        .call_tool("echo", arguments.as_object().cloned())
        .await
        .expect("the call answers");

    let Some(rmcp::model::ContentBlock::Text(text)) = result.content.first() else {
        panic!("the server answers in text: {:?}", result.content);
    };
    assert_eq!(text.text, "Echo: hello");

    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn refuses_a_header_name_that_is_not_one() {
    let endpoint = HttpEndpoint::new("http://127.0.0.1:1/mcp").header("not a header", "x");
    let error = Client::connect(Transport::Http(endpoint))
        .await
        .err()
        .expect("connect should fail");

    assert!(
        matches!(error, McpError::Endpoint(_)),
        "the name is refused before anything is sent: {error:?}"
    );
}

#[tokio::test]
async fn a_server_that_refuses_the_token_is_not_connected() {
    let server = MockServer::start();
    // This endpoint carries no bearer token, and the mock server answers 401
    // without one.
    let endpoint = HttpEndpoint::new(&server.url);

    let connected = Client::connect(Transport::Http(endpoint)).await;

    assert!(
        connected.is_err(),
        "a server that refuses the handshake is not connected"
    );
}
