//! Connects to a real child process and reads the tools it declares.

use rmcp::model::ContentBlock;
use serde_json::json;
use srud_mcp::{Client, McpError, ServerCommand, Transport};

fn mock_server() -> Transport {
    ServerCommand::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/mock_server.py"
        ))
        .into()
}

#[tokio::test]
async fn connects_and_lists_tools() {
    let client = Client::connect(mock_server()).await.expect("connect");
    assert_eq!(client.server_name().as_deref(), Some("mock"));

    let tools = client.list_tools().await.expect("list tools");
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(names, ["echo", "add"]);

    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn carries_the_description_and_schema() {
    let client = Client::connect(mock_server()).await.expect("connect");
    let tools = client.list_tools().await.expect("list tools");
    let echo = tools
        .iter()
        .find(|tool| tool.name == "echo")
        .expect("echo tool");

    assert_eq!(echo.description.as_deref(), Some("Echo the text back"));
    assert!(echo.input_schema.contains_key("properties"));

    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn reports_a_server_that_cannot_start() {
    let error = Client::connect(ServerCommand::new("/nonexistent/mcp-server").into())
        .await
        .err()
        .expect("connect should fail");

    assert!(matches!(error, McpError::Spawn(_)));
}

#[tokio::test]
async fn calls_a_tool() {
    let client = Client::connect(mock_server()).await.expect("connect");
    let arguments = json!({ "text": "hello" });
    let result = client
        .call_tool("echo", arguments.as_object().cloned())
        .await
        .expect("the call answers");

    let Some(ContentBlock::Text(text)) = result.content.first() else {
        panic!("the server answers in text: {:?}", result.content);
    };
    assert_eq!(text.text, "Echo: hello");
    assert_eq!(result.is_error, None);
    assert!(result.structured_content.is_none());

    client.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn passes_back_a_tool_that_failed() {
    let client = Client::connect(mock_server()).await.expect("connect");
    let arguments = json!({ "a": -1, "b": 2 });
    let result = client
        .call_tool("add", arguments.as_object().cloned())
        .await
        .expect("a failed tool still answers");

    assert_eq!(result.is_error, Some(true));
    let Some(ContentBlock::Text(text)) = result.content.first() else {
        panic!("the refusal is text: {:?}", result.content);
    };
    assert!(
        text.text.contains("non-negative"),
        "the server's reason reaches the caller: {}",
        text.text
    );

    client.shutdown().await.expect("shutdown");
}
