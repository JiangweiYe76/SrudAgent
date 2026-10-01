//! Tool definitions and dispatch.
//!
//! A tool is a pure function of its arguments: it does not hold a session and
//! cannot append to history. A tool that ran and failed returns
//! [`ToolOutcome::failure`]; an `Err` means the runtime itself could not
//! proceed.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

/// What a tool produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutcome {
    /// Text handed back to the model.
    pub output: String,
    /// Whether the tool ran and failed.
    pub is_error: bool,
    /// File paths the tool touched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub touched_paths: Vec<String>,
}

impl ToolOutcome {
    /// Builds a successful outcome.
    #[must_use]
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            touched_paths: Vec::new(),
        }
    }

    /// Builds a failed outcome.
    #[must_use]
    pub fn failure(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            touched_paths: Vec::new(),
        }
    }

    /// Attaches the paths this tool touched.
    #[must_use]
    pub fn with_paths<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.touched_paths = paths.into_iter().map(Into::into).collect();
        self
    }
}

/// A tool the model can call.
///
/// Implementations must be `Send + Sync` because the registry is shared across
/// turns, and should avoid blocking.
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    /// The name the model uses to call this tool.
    fn name(&self) -> &str;

    /// A description shown to the model.
    fn description(&self) -> &str;

    /// JSON Schema for the arguments object.
    fn parameters(&self) -> serde_json::Value;

    /// Runs the tool.
    ///
    /// Returns [`ToolOutcome::failure`] for expected failures.
    async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, ToolError>;
}

/// A description of a tool, used when building a model request.
#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    /// Tool name.
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// JSON Schema for the arguments.
    pub parameters: serde_json::Value,
}

/// Something went wrong at the runtime level, not inside the tool.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// A tool with this name is not registered.
    #[error("no such tool: {0}")]
    NotFound(String),

    /// Arguments could not be parsed or did not match the schema.
    #[error("invalid arguments for tool {name}: {message}")]
    InvalidArguments {
        /// The tool that rejected the arguments.
        name: String,
        /// Why they were rejected.
        message: String,
    },
}

/// A tool could not be registered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("a tool named `{0}` is already registered")]
pub struct DuplicateTool(pub String);

/// The set of tools available to the loop.
///
/// Definitions are handed to the model sorted by name, so requests are stable
/// across runs.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, Arc<dyn Tool>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRegistry")
            .field("tools", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ToolRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a tool.
    ///
    /// # Errors
    ///
    /// Returns [`DuplicateTool`] if the name is already taken. The name is what
    /// the model calls, so silently replacing a tool would change what an
    /// existing call means.
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> Result<(), DuplicateTool> {
        let name = tool.name().to_owned();
        if self.tools.contains_key(&name) {
            return Err(DuplicateTool(name));
        }
        self.tools.insert(name, tool);
        Ok(())
    }

    /// Returns whether a tool with this name exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Returns how many tools are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Returns whether the registry is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Returns definitions for every registered tool, sorted by name.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters: tool.parameters(),
            })
            .collect()
    }

    /// Runs a tool by name.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::NotFound`] if the name is unknown. A tool that ran
    /// and failed returns `Ok(`[`ToolOutcome::failure`]`)`.
    pub async fn dispatch(
        &self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let tool = self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_owned()))?;
        tool.call(arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Echo;

    #[async_trait::async_trait]
    impl Tool for Echo {
        fn name(&self) -> &str {
            "echo"
        }
        fn description(&self) -> &str {
            "Returns the `text` argument unchanged."
        }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"],
            })
        }
        async fn call(&self, arguments: serde_json::Value) -> Result<ToolOutcome, ToolError> {
            match arguments.get("text").and_then(|v| v.as_str()) {
                Some(text) => Ok(ToolOutcome::success(text)),
                None => Err(ToolError::InvalidArguments {
                    name: "echo".into(),
                    message: "missing `text`".into(),
                }),
            }
        }
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(Echo)).expect("fresh registry");
        registry
    }

    #[tokio::test]
    async fn dispatches_to_a_registered_tool() {
        let registry = registry();
        let outcome = registry
            .dispatch("echo", serde_json::json!({ "text": "hi" }))
            .await
            .expect("registered");
        assert_eq!(outcome.output, "hi");
        assert!(!outcome.is_error);
    }

    #[tokio::test]
    async fn unknown_tool_is_a_runtime_error() {
        let registry = registry();
        let err = registry
            .dispatch("nope", serde_json::json!({}))
            .await
            .expect_err("unregistered");
        assert!(matches!(err, ToolError::NotFound(name) if name == "nope"));
    }

    #[tokio::test]
    async fn bad_arguments_surface_from_the_tool() {
        let registry = registry();
        let err = registry
            .dispatch("echo", serde_json::json!({}))
            .await
            .expect_err("missing text");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn duplicate_names_are_refused() {
        let mut registry = registry();
        assert!(registry.register(Arc::new(Echo)).is_err());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn definitions_are_sorted_by_name() {
        struct A;
        #[async_trait::async_trait]
        impl Tool for A {
            fn name(&self) -> &str {
                "aaa"
            }
            fn description(&self) -> &str {
                "first"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({ "type": "object" })
            }
            async fn call(&self, _a: serde_json::Value) -> Result<ToolOutcome, ToolError> {
                Ok(ToolOutcome::success(""))
            }
        }

        let mut registry = registry();
        registry.register(Arc::new(A)).expect("unique name");

        let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["aaa", "echo"]);
    }
}
