//! What a server declares, as the tools the agent's registry takes.

use std::sync::Arc;

use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use srud_core::tools::{failure_text, Tool as AgentTool, ToolContext, ToolError, ToolOutcome};

use crate::client::Client;
use crate::error::McpError;

/// The prefix every tool a server declares is named with.
///
/// A name the model already knows — `read`, `write`, `shell` — must not be
/// shadowed by whatever a server happens to call its tool, and two servers may
/// each declare a `search`. The server's own name in the middle keeps both
/// apart without the agent having to decide which one wins.
pub const TOOL_PREFIX: &str = "mcp__";

/// Connects one server and returns the tools it declares, named for the model.
///
/// The tools hold the connection: it stays open, and a server started here keeps
/// running, for as long as any of them is alive. A server that declares no tools
/// leaves nothing to hold it, so it is closed at once — it has nothing to offer.
///
/// # Errors
///
/// As [`Client::connect`] and [`Client::list_tools`].
pub async fn connect_tools(
    server: &str,
    transport: impl Into<crate::transport::Transport>,
) -> Result<Vec<Arc<dyn AgentTool>>, McpError> {
    let client = Arc::new(Client::connect(transport.into()).await?);
    let tools = client.list_tools().await?;
    Ok(tools
        .into_iter()
        .map(|tool| {
            Arc::new(McpTool {
                client: Arc::clone(&client),
                server: server.to_owned(),
                name: tool_name(server, tool.name.as_ref()),
                remote: tool.name.to_string(),
                description: tool.description.as_deref().unwrap_or_default().to_owned(),
                parameters: serde_json::Value::Object((*tool.input_schema).clone()),
            }) as Arc<dyn AgentTool>
        })
        .collect())
}

/// One tool a connected server declares, as the agent's tool set sees it.
struct McpTool {
    client: Arc<Client>,
    /// The server's own name, as the client sent it.
    server: String,
    /// The name the model calls this tool by.
    name: String,
    /// The name the server knows it by.
    remote: String,
    description: String,
    parameters: serde_json::Value,
}

#[async_trait::async_trait]
impl AgentTool for McpTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        self.parameters.clone()
    }

    async fn call(
        &self,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let arguments = match arguments {
            serde_json::Value::Object(arguments) => arguments,
            serde_json::Value::Null => JsonObject::new(),
            other => {
                return Err(ToolError::InvalidArguments {
                    name: self.name.clone(),
                    message: format!("arguments must be an object, got {other}"),
                })
            }
        };

        let called = tokio::select! {
            result = self.client.call_tool(&self.remote, Some(arguments)) => result,
            () = ctx.cancel.cancelled() => {
                return Ok(ToolOutcome::failure(failure_text(
                    format!("`{}` was interrupted before the server answered.", self.name),
                    None,
                )));
            }
        };

        // A server that could not be reached, or that answered with something
        // this client cannot finish, is a failed call rather than a broken
        // runtime: the turn goes on and the model is told what happened.
        match called {
            Ok(result) => Ok(outcome_of(result)),
            Err(err) => Ok(ToolOutcome::failure(failure_text(
                format!("the MCP server `{}` did not answer: {err}", self.server),
                None,
            ))),
        }
    }
}

/// Turns what a server returned into the text the model reads.
fn outcome_of(result: CallToolResult) -> ToolOutcome {
    let mut parts: Vec<String> = result
        .content
        .iter()
        .map(|block| match block {
            ContentBlock::Text(text) => text.text.clone(),
            ContentBlock::Image(image) => format!("[image: {}]", image.mime_type),
            ContentBlock::Audio(audio) => format!("[audio: {}]", audio.mime_type),
            ContentBlock::Resource(resource) => match &resource.resource {
                rmcp::model::ResourceContents::TextResourceContents { text, .. } => text.clone(),
                rmcp::model::ResourceContents::BlobResourceContents { uri, .. } => {
                    format!("[resource: {uri}]")
                }
                _ => "[resource]".to_owned(),
            },
            ContentBlock::ResourceLink(link) => format!("{} ({})", link.uri, link.name),
            _ => "[content this client cannot show]".to_owned(),
        })
        .collect();
    // A tool that answers only in structured content still has to say something:
    // the model reads text, not the envelope.
    if parts.is_empty() {
        if let Some(structured) = &result.structured_content {
            parts.push(structured.to_string());
        }
    }
    let output = parts.join("\n");
    // A refusal is marked the way every other failed tool marks one: the model
    // reads the text, not the `is_error` flag beside it.
    if result.is_error.unwrap_or(false) {
        ToolOutcome::failure(failure_text(output, None))
    } else {
        ToolOutcome::success(output)
    }
}

/// The name the model calls a server's tool by: `mcp__<server>__<tool>`.
///
/// Only characters a model reliably reproduces are kept; anything else becomes
/// `_`, so a server named `my server` still yields a name with no spaces in it.
fn tool_name(server: &str, tool: &str) -> String {
    let server: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{TOOL_PREFIX}{server}__{tool}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use srud_core::tools::FAILURE_MARKER;

    #[test]
    fn names_a_tool_after_its_server() {
        assert_eq!(tool_name("docs", "echo"), "mcp__docs__echo");
        assert_eq!(tool_name("my-docs_2", "read"), "mcp__my-docs_2__read");
    }

    #[test]
    fn replaces_what_a_model_cannot_reproduce() {
        assert_eq!(tool_name("my server", "echo"), "mcp__my_server__echo");
        assert_eq!(tool_name("a/b", "echo"), "mcp__a_b__echo");
        // One underscore per character kept out, so a two-character name leaves
        // four between the prefix and the tool's own name.
        assert_eq!(tool_name("文档", "echo"), "mcp______echo");
    }

    #[test]
    fn joins_text_blocks_in_order() {
        let outcome = outcome_of(CallToolResult::success(vec![
            ContentBlock::text("first"),
            ContentBlock::text("second"),
        ]));

        assert_eq!(outcome.output, "first\nsecond");
        assert!(!outcome.is_error);
    }

    #[test]
    fn names_content_the_model_cannot_read() {
        let outcome = outcome_of(CallToolResult::success(vec![
            ContentBlock::image("Zm9v", "image/png"),
            ContentBlock::audio("YmFy", "audio/wav"),
        ]));

        assert_eq!(outcome.output, "[image: image/png]\n[audio: audio/wav]");
    }

    #[test]
    fn falls_back_to_structured_content() {
        let mut result = CallToolResult::success(Vec::new());
        result.structured_content = Some(json!({ "count": 2 }));

        let outcome = outcome_of(result);

        assert_eq!(outcome.output, r#"{"count":2}"#);
        assert!(!outcome.is_error);
    }

    #[test]
    fn marks_a_refusal_the_way_every_failed_tool_is_marked() {
        let outcome = outcome_of(CallToolResult::error(vec![ContentBlock::text(
            "no such file",
        )]));

        assert!(outcome.is_error);
        assert!(
            outcome.output.starts_with(FAILURE_MARKER),
            "the model reads the failure from the text: {}",
            outcome.output
        );
        assert!(outcome.output.contains("no such file"));
    }
}
