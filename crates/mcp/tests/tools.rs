//! The tools a connected server declares, as the agent's registry sees them.

use serde_json::json;
use srud_core::tools::{ToolContext, ToolRegistry};
use srud_mcp::{connect_tools, ServerCommand};
use tokio_util::sync::CancellationToken;

fn mock_server() -> ServerCommand {
    ServerCommand::new("python3").arg(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/mock_server.py"
    ))
}

fn context() -> ToolContext {
    ToolContext {
        cwd: std::env::temp_dir(),
        cancel: CancellationToken::new(),
        sandbox: srud_core::sandbox::SandboxPolicy::Unrestricted,
    }
}

/// The mock server's tools, registered the way the agent registers them.
async fn registry() -> ToolRegistry {
    let tools = connect_tools("docs", mock_server()).await.expect("connect");
    ToolRegistry::new()
        .with_tools(tools)
        .expect("the mock server declares no duplicate names")
}

#[tokio::test]
async fn names_each_tool_after_its_server() {
    let names: Vec<String> = registry()
        .await
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();

    assert_eq!(names, ["mcp__docs__add", "mcp__docs__echo"]);
}

#[tokio::test]
async fn keeps_the_name_reproducible_when_the_server_has_spaces_in_it() {
    let tools = connect_tools("my server", mock_server())
        .await
        .expect("connect");
    let names: Vec<String> = tools.iter().map(|tool| tool.name().to_owned()).collect();

    assert!(
        names
            .iter()
            .all(|name| name.starts_with("mcp__my_server__")),
        "a name the model reproduces has no spaces in it: {names:?}"
    );
}

#[tokio::test]
async fn carries_the_description_and_the_schema() {
    let registry = registry().await;
    let echo = registry
        .definitions()
        .into_iter()
        .find(|definition| definition.name == "mcp__docs__echo")
        .expect("the echo tool is registered");

    assert_eq!(echo.description, "Echo the text back");
    assert_eq!(
        echo.parameters["properties"]["text"]["type"],
        json!("string")
    );
}

#[tokio::test]
async fn calls_the_server_and_returns_its_text() {
    let registry = registry().await;
    let outcome = registry
        .dispatch("mcp__docs__echo", &context(), json!({ "text": "hello" }))
        .await
        .expect("the tool is registered");

    assert_eq!(outcome.output, "Echo: hello");
    assert!(!outcome.is_error);
}

#[tokio::test]
async fn keeps_a_failed_call_a_tool_error() {
    let registry = registry().await;
    let outcome = registry
        .dispatch("mcp__docs__add", &context(), json!({ "a": -1, "b": 2 }))
        .await
        .expect("the tool is registered");

    assert!(outcome.is_error, "the server's refusal is marked as one");
    assert!(
        outcome.output.starts_with(srud_core::tools::FAILURE_MARKER),
        "the model reads the failure from the text: {}",
        outcome.output
    );
    assert!(
        outcome.output.contains("non-negative"),
        "the server's reason reaches the model: {}",
        outcome.output
    );
}

#[tokio::test]
async fn refuses_arguments_that_are_not_an_object() {
    let registry = registry().await;
    let error = registry
        .dispatch("mcp__docs__echo", &context(), json!("text"))
        .await
        .expect_err("a bare string is not a call");

    assert!(
        matches!(error, srud_core::tools::ToolError::InvalidArguments { .. }),
        "the refusal names the arguments rather than the tool: {error:?}"
    );
}
