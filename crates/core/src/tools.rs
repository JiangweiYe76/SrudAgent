//! Tool definitions and dispatch.
//!
//! A tool is a pure function of its arguments and the context it is handed: it
//! does not hold a session and cannot append to history. A tool that ran and
//! failed returns [`ToolOutcome::failure`]; an `Err` means the runtime itself
//! could not proceed.

pub mod edit;
pub mod grep;
pub mod read;
pub mod shell;
pub mod skill;
pub mod write;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use serde::{Deserialize, Serialize};

use srud_sandbox::SandboxPolicy;

/// What a tool needs to know about the run it is part of.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// The session's working directory. A tool that takes a relative path
    /// resolves it against this.
    pub cwd: PathBuf,
    /// The turn's cancellation token. A tool that runs long enough to notice
    /// should stop when it fires.
    pub cancel: CancellationToken,
    /// The boundary this turn runs under.
    ///
    /// Full access is [`SandboxPolicy::Unrestricted`]: a tool sees no OS boundary
    /// and imposes none of its own.
    pub sandbox: SandboxPolicy,
}

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

/// How a failed tool is marked in the text the model reads.
///
/// The provider carries tool output as a string, so `is_error` reaches the
/// desktop UI but not the model: the text is the only place a failure can be
/// recognised. A prefix is what lets a model tell a refusal from a result whose
/// `exit_code` is 1, instead of inferring it from the prose.
pub const FAILURE_MARKER: &str = "[tool error]";

/// Resolves a path a tool was asked for: an absolute one is taken as given, a
/// relative one is anchored to the working directory.
///
/// Shared rather than repeated, so a bare file name means the same thing to every
/// tool that takes one — a name `read` resolved is a name `write` can open.
#[must_use]
pub(crate) fn resolve(ctx: &ToolContext, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    }
}

/// Refuses a write outside the roots a bounded session may modify.
///
/// `None` when the write is allowed, including every write in an unrestricted
/// session. The refusal is returned as the text a tool hands back, so every
/// file-writing tool reports the same boundary the same way.
#[must_use]
pub(crate) fn denied_write(ctx: &ToolContext, path: &Path) -> Option<String> {
    let SandboxPolicy::Bounded(policy) = &ctx.sandbox else {
        return None;
    };
    if policy.allows_write(path) {
        return None;
    }
    let shown = path.display();
    Some(failure_text(
        format!("Cannot write {shown}: it is outside this session's writable roots."),
        Some(
            "Nothing was written. This session writes inside its workspace and temporary \
             directories only; do not try to reach the path another way.",
        ),
    ))
}

/// The error for arguments a tool's schema permits but it cannot act on.
pub(crate) fn invalid_arguments(name: &str, message: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments {
        name: name.to_owned(),
        message: message.into(),
    }
}

/// A boolean argument, tolerant of the two ways a model quotes one.
///
/// Models sometimes send `"false"` where the schema says boolean, and a plain
/// `bool` answers that with a type error naming the shape rather than the
/// mistake. Reading it as truthiness instead would be worse: it turns `"false"`
/// into `true`, so the tool would do the more destructive of the two things the
/// argument chooses between.
///
/// Only the string literals are accepted, and the tolerance stops at
/// deserialization — [`Tool::parameters`] still says `boolean`, so the model is
/// told the real shape and nothing invites it to quote the value.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LenientBool(pub bool);

impl<'de> serde::Deserialize<'de> for LenientBool {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Quoted {
            Bare(bool),
            Text(String),
        }

        match Quoted::deserialize(deserializer)? {
            Quoted::Bare(value) => Ok(Self(value)),
            Quoted::Text(text) => match text.as_str() {
                "true" => Ok(Self(true)),
                "false" => Ok(Self(false)),
                other => Err(serde::de::Error::custom(format!(
                    "`{other}` is not a boolean; send true or false, unquoted"
                ))),
            },
        }
    }
}

/// Builds the text a failed tool hands back.
///
/// Every failure goes through here, so a model meets the same marker whatever
/// failed. `hint` is what the caller can add that the error alone cannot say —
/// which argument was wrong, or what to try instead.
#[must_use]
pub fn failure_text(message: impl AsRef<str>, hint: Option<&str>) -> String {
    let mut text = format!("{FAILURE_MARKER} {}", message.as_ref());
    if let Some(hint) = hint {
        text.push_str("\n\n");
        text.push_str(hint);
    }
    text
}

/// The outcome a tool reports when the turn is stopped while it runs.
///
/// A failed outcome rather than an error: the call was dispatched, so the log
/// has to pair it with a result, and the model reads what happened from the
/// text. `hint` says what the tool left undone, which only the tool knows.
pub(crate) fn stopped(hint: &str) -> ToolOutcome {
    ToolOutcome::failure(failure_text("Stopped by the user.", Some(hint)))
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
    ///
    /// Sent to the model, and nothing reads it back: no part of the runtime
    /// checks arguments against this schema. A constraint written here is a
    /// request to the model, not a rule, so a tool that cannot run an argument it
    /// declared has to say so in [`Tool::validate`].
    fn parameters(&self) -> serde_json::Value;

    /// Checks the arguments before the tool runs.
    ///
    /// Separate from [`Tool::call`] so a refusal costs no side effect: whatever
    /// runs first has already spent a round trip by the time it notices. The
    /// default accepts everything, which is right for a tool whose only
    /// constraints are its argument types — those the deserialization in `call`
    /// already enforces.
    ///
    /// Every problem with the arguments is reported at once, naming each by its
    /// path, so the model can fix them in one call rather than one per turn.
    fn validate(&self, _arguments: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// Runs the tool.
    ///
    /// Returns [`ToolOutcome::failure`] for expected failures.
    async fn call(
        &self,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError>;
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

/// Where a registered tool came from.
///
/// Recorded on the registration rather than on the tool itself: the same
/// implementation may be offered by more than one source, and a source describes
/// the relationship, not what the tool does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolSource {
    /// Compiled into the agent.
    Builtin,
    /// Declared by an MCP server, named as the client names it.
    Mcp(String),
    /// Contributed by a plugin, named as the plugin names it.
    Plugin(String),
}

/// How far a tool reaches outside the conversation.
///
/// Recorded so a policy can decide what to allow without inspecting an
/// implementation, and so a client can describe a tool before calling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Risk {
    /// Reads state without changing it.
    ReadOnly,
    /// Changes files.
    Writes,
    /// Runs a program.
    Executes,
    /// Reaches the network.
    Network,
    /// Not declared by its source.
    Unknown,
}

/// One tool, named by the source that offers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolId {
    /// The source that offers this tool.
    pub source: ToolSource,
    /// The source's own name for it.
    pub local: String,
}

impl std::fmt::Display for ToolId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.source {
            ToolSource::Builtin => write!(f, "builtin/{}", self.local),
            ToolSource::Mcp(server) => write!(f, "mcp:{server}/{}", self.local),
            ToolSource::Plugin(plugin) => write!(f, "plugin:{plugin}/{}", self.local),
        }
    }
}

impl std::str::FromStr for ToolId {
    type Err = InvalidToolId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (prefix, local) = text
            .split_once('/')
            .ok_or_else(|| InvalidToolId(text.to_owned()))?;
        if local.is_empty() || local.contains('/') {
            return Err(InvalidToolId(text.to_owned()));
        }

        let source = if prefix == "builtin" {
            ToolSource::Builtin
        } else {
            let (kind, name) = prefix
                .split_once(':')
                .ok_or_else(|| InvalidToolId(text.to_owned()))?;
            if name.is_empty() || name.contains('/') {
                return Err(InvalidToolId(text.to_owned()));
            }
            match kind {
                "mcp" => ToolSource::Mcp(name.to_owned()),
                "plugin" => ToolSource::Plugin(name.to_owned()),
                _ => return Err(InvalidToolId(text.to_owned())),
            }
        };

        Ok(Self {
            source,
            local: local.to_owned(),
        })
    }
}

impl Serialize for ToolId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ToolId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Why a string is not a tool id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "`{0}` is not a tool id; write builtin/<name>, mcp:<server>/<name> or plugin:<plugin>/<name>"
)]
pub struct InvalidToolId(pub String);

/// What a registered tool is, without the schema of its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    /// Where the tool came from and its local name.
    pub id: ToolId,
    /// The name the model calls it by.
    pub name: String,
    /// What the source says the tool does.
    pub description: String,
    /// How far it can reach.
    pub risk: Risk,
}

/// One tool as the registry holds it: what it is, and how to run it.
#[derive(Clone)]
pub struct RegisteredTool {
    descriptor: ToolDescriptor,
    tool: Arc<dyn Tool>,
}

impl RegisteredTool {
    /// Registers a tool under the source's own name for it.
    #[must_use]
    pub fn new(
        source: ToolSource,
        local: impl Into<String>,
        risk: Risk,
        tool: Arc<dyn Tool>,
    ) -> Self {
        let descriptor = ToolDescriptor {
            id: ToolId {
                source,
                local: local.into(),
            },
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            risk,
        };
        Self { descriptor, tool }
    }

    /// Registers a builtin tool under the name it gives itself.
    #[must_use]
    pub fn builtin(risk: Risk, tool: Arc<dyn Tool>) -> Self {
        let local = tool.name().to_owned();
        Self::new(ToolSource::Builtin, local, risk, tool)
    }

    /// What the registry knows about this tool.
    #[must_use]
    pub fn descriptor(&self) -> &ToolDescriptor {
        &self.descriptor
    }

    /// The source that contributed this tool.
    #[must_use]
    pub fn source(&self) -> &ToolSource {
        &self.descriptor.id.source
    }

    /// The tool itself.
    #[must_use]
    pub fn tool(&self) -> &Arc<dyn Tool> {
        &self.tool
    }
}

impl std::fmt::Debug for RegisteredTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisteredTool")
            .field("descriptor", &self.descriptor)
            .finish()
    }
}

/// What a policy decided about the tools offered to one session.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    /// The tools that survived, one per model-visible name.
    pub tools: Vec<RegisteredTool>,
    /// What the policy dropped or shadowed on the way.
    pub diagnostics: Vec<Diagnostic>,
}

/// One tool the policy left out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// The tool was on the deny list.
    Denied {
        /// The tool that was refused.
        id: ToolId,
    },
    /// Two tools wanted the same model-visible name.
    Shadowed {
        /// The name both wanted.
        name: String,
        /// The tool that kept it.
        winner: ToolId,
        /// The tool that lost it.
        loser: ToolId,
    },
}

/// Which offered tools a session keeps.
///
/// Applied after every source has contributed. The defaults keep everything and
/// give a builtin tool precedence over a plugin or an MCP server offering the
/// same model-visible name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPolicy {
    /// Tools refused even when their source offers them.
    pub deny: Vec<ToolId>,
    /// Higher-precedence tools a lower-precedence source may replace by name.
    pub overridable: Vec<ToolId>,
}

impl ToolPolicy {
    /// Applies the policy to the tools a session was offered.
    #[must_use]
    pub fn select(&self, candidates: Vec<RegisteredTool>) -> Selection {
        let mut selected: BTreeMap<String, RegisteredTool> = BTreeMap::new();
        let mut diagnostics = Vec::new();

        for candidate in candidates {
            let id = candidate.descriptor().id.clone();
            if self.deny.contains(&id) {
                diagnostics.push(Diagnostic::Denied { id });
                continue;
            }

            let name = candidate.descriptor().name.clone();
            let Some(existing) = selected.get(&name) else {
                selected.insert(name, candidate);
                continue;
            };

            if self.replaces(existing, &candidate) {
                let winner = candidate.descriptor().id.clone();
                let replaced = selected.insert(name.clone(), candidate).expect("present");
                diagnostics.push(Diagnostic::Shadowed {
                    name,
                    winner,
                    loser: replaced.descriptor().id.clone(),
                });
            } else {
                diagnostics.push(Diagnostic::Shadowed {
                    name,
                    winner: existing.descriptor().id.clone(),
                    loser: candidate.descriptor().id.clone(),
                });
            }
        }

        Selection {
            tools: selected.into_values().collect(),
            diagnostics,
        }
    }

    /// Whether a candidate takes the name from the tool that holds it now.
    fn replaces(&self, existing: &RegisteredTool, candidate: &RegisteredTool) -> bool {
        let existing_rank = precedence(existing.source());
        let candidate_rank = precedence(candidate.source());
        match candidate_rank.cmp(&existing_rank) {
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => self.overridable.contains(&existing.descriptor().id),
            std::cmp::Ordering::Equal => false,
        }
    }
}

/// Lower numbers win when two tools want the same name.
fn precedence(source: &ToolSource) -> usize {
    match source {
        ToolSource::Builtin => 0,
        ToolSource::Plugin(_) => 1,
        ToolSource::Mcp(_) => 2,
    }
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
/// Each entry records the source that contributed it. Definitions are handed to
/// the model sorted by name, so requests are stable across runs.
#[derive(Default)]
pub struct ToolRegistry {
    tools: BTreeMap<String, RegisteredTool>,
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

    /// Adds a tool that already names its source.
    ///
    /// # Errors
    ///
    /// Returns [`DuplicateTool`] if the name is already taken. The name is what
    /// the model calls, so silently replacing a tool would change what an
    /// existing call means.
    pub fn register(&mut self, registered: RegisteredTool) -> Result<(), DuplicateTool> {
        let name = registered.tool.name().to_owned();
        if self.tools.contains_key(&name) {
            return Err(DuplicateTool(name));
        }
        self.tools.insert(name, registered);
        Ok(())
    }

    /// Returns whether a tool with this name exists.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Returns the entry registered under this name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&RegisteredTool> {
        self.tools.get(name)
    }

    /// Returns every entry, sorted by model-visible name.
    #[must_use]
    pub fn entries(&self) -> Vec<RegisteredTool> {
        self.tools.values().cloned().collect()
    }

    /// Builds a registry from tools, refusing a name claimed twice.
    ///
    /// # Errors
    ///
    /// Returns [`DuplicateTool`] naming the first name claimed twice.
    pub fn from_tools<I>(tools: I) -> Result<Self, DuplicateTool>
    where
        I: IntoIterator<Item = RegisteredTool>,
    {
        let mut registry = Self::new();
        for tool in tools {
            registry.register(tool)?;
        }
        Ok(registry)
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
            .map(|registered| ToolDefinition {
                name: registered.tool.name().to_owned(),
                description: registered.tool.description().to_owned(),
                parameters: registered.tool.parameters(),
            })
            .collect()
    }

    /// Returns what the registry knows about every tool, sorted by name.
    #[must_use]
    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools
            .values()
            .map(|registered| registered.descriptor.clone())
            .collect()
    }

    /// Runs a tool by name.
    ///
    /// # Errors
    ///
    /// Returns [`ToolError::NotFound`] if the name is unknown, and
    /// [`ToolError::InvalidArguments`] if the tool refuses its arguments.
    /// A tool that ran and failed returns `Ok(`[`ToolOutcome::failure`]`)`.
    pub async fn dispatch(
        &self,
        name: &str,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let tool = &self
            .tools
            .get(name)
            .ok_or_else(|| ToolError::NotFound(name.to_owned()))?
            .tool;
        if let Err(problems) = tool.validate(&arguments) {
            return Err(ToolError::InvalidArguments {
                name: name.to_owned(),
                message: problems,
            });
        }
        tool.call(ctx, arguments).await
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
        async fn call(
            &self,
            _ctx: &ToolContext,
            arguments: serde_json::Value,
        ) -> Result<ToolOutcome, ToolError> {
            match arguments.get("text").and_then(|v| v.as_str()) {
                Some(text) => Ok(ToolOutcome::success(text)),
                None => Err(ToolError::InvalidArguments {
                    name: "echo".into(),
                    message: "missing `text`".into(),
                }),
            }
        }
    }

    /// The context these tests hand to tools; none of them look at it.
    fn ctx() -> ToolContext {
        ToolContext {
            cwd: PathBuf::from("workspace"),
            cancel: tokio_util::sync::CancellationToken::new(),
            sandbox: crate::sandbox::SandboxPolicy::Unrestricted,
        }
    }

    fn registry() -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        registry
            .register(RegisteredTool::builtin(Risk::Unknown, Arc::new(Echo)))
            .expect("fresh registry");
        registry
    }

    #[tokio::test]
    async fn dispatches_to_a_registered_tool() {
        let registry = registry();
        let outcome = registry
            .dispatch("echo", &ctx(), serde_json::json!({ "text": "hi" }))
            .await
            .expect("registered");
        assert_eq!(outcome.output, "hi");
        assert!(!outcome.is_error);
    }

    #[tokio::test]
    async fn unknown_tool_is_a_runtime_error() {
        let registry = registry();
        let err = registry
            .dispatch("nope", &ctx(), serde_json::json!({}))
            .await
            .expect_err("unregistered");
        assert!(matches!(err, ToolError::NotFound(name) if name == "nope"));
    }

    #[tokio::test]
    async fn bad_arguments_surface_from_the_tool() {
        let registry = registry();
        let err = registry
            .dispatch("echo", &ctx(), serde_json::json!({}))
            .await
            .expect_err("missing text");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn duplicate_names_are_refused() {
        let mut registry = registry();
        assert!(registry
            .register(RegisteredTool::builtin(Risk::Unknown, Arc::new(Echo)))
            .is_err());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn a_registered_tool_keeps_its_id_and_risk() {
        let registry = registry();
        let descriptor = registry.get("echo").expect("registered").descriptor();

        assert_eq!(
            descriptor.id,
            ToolId {
                source: ToolSource::Builtin,
                local: "echo".into(),
            }
        );
        assert_eq!(descriptor.risk, Risk::Unknown);
    }

    #[test]
    fn from_tools_keeps_each_tools_source() {
        struct Other;

        #[async_trait::async_trait]
        impl Tool for Other {
            fn name(&self) -> &str {
                "other"
            }
            fn description(&self) -> &str {
                "second"
            }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({ "type": "object" })
            }
            async fn call(
                &self,
                _ctx: &ToolContext,
                _arguments: serde_json::Value,
            ) -> Result<ToolOutcome, ToolError> {
                Ok(ToolOutcome::success(""))
            }
        }

        let merged = ToolRegistry::from_tools([
            RegisteredTool::builtin(Risk::Unknown, Arc::new(Echo)),
            RegisteredTool::new(
                ToolSource::Mcp("docs".into()),
                "other",
                Risk::Unknown,
                Arc::new(Other),
            ),
        ])
        .expect("the names differ");

        assert_eq!(
            merged.get("echo").expect("registered").source(),
            &ToolSource::Builtin
        );
        assert_eq!(
            merged.get("other").expect("registered").source(),
            &ToolSource::Mcp("docs".into())
        );
    }

    #[test]
    fn a_tool_id_round_trips_through_its_text_form() {
        for text in ["builtin/read", "mcp:docs/echo", "plugin:docs/echo"] {
            let id: ToolId = text.parse().expect("a valid id");
            assert_eq!(id.to_string(), text);

            let json = serde_json::to_string(&id).expect("an id serialises");
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(
                serde_json::from_str::<ToolId>(&json).expect("an id reads back"),
                id
            );
        }
    }

    #[test]
    fn a_bad_tool_id_is_refused() {
        for text in [
            "",
            "read",
            "builtin/",
            "builtin/read/extra",
            "mcp:/echo",
            "mcp:docs",
            "unknown/read",
        ] {
            assert!(text.parse::<ToolId>().is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn a_policy_reads_from_json() {
        let policy: ToolPolicy =
            serde_json::from_str(r#"{"deny":["builtin/shell"],"overridable":["builtin/read"]}"#)
                .expect("a valid policy");

        assert_eq!(policy.deny[0].to_string(), "builtin/shell");
        assert_eq!(policy.overridable[0].to_string(), "builtin/read");
    }

    #[test]
    fn a_policy_refuses_an_unknown_key() {
        assert!(serde_json::from_str::<ToolPolicy>(r#"{"denny":["builtin/shell"]}"#).is_err());
    }

    #[test]
    fn a_denied_tool_is_left_out() {
        let policy = ToolPolicy {
            deny: vec![ToolId {
                source: ToolSource::Builtin,
                local: "echo".into(),
            }],
            ..ToolPolicy::default()
        };

        let selection = policy.select(vec![RegisteredTool::builtin(Risk::Unknown, Arc::new(Echo))]);

        assert!(selection.tools.is_empty());
        assert_eq!(
            selection.diagnostics,
            vec![Diagnostic::Denied {
                id: ToolId {
                    source: ToolSource::Builtin,
                    local: "echo".into(),
                }
            }]
        );
    }

    #[test]
    fn a_builtin_wins_a_name_clash_by_default() {
        let policy = ToolPolicy::default();
        let selection = policy.select(vec![
            RegisteredTool::new(
                ToolSource::Mcp("docs".into()),
                "echo",
                Risk::Unknown,
                Arc::new(Echo),
            ),
            RegisteredTool::builtin(Risk::Unknown, Arc::new(Echo)),
        ]);

        assert_eq!(selection.tools.len(), 1);
        assert_eq!(
            selection.tools[0].source(),
            &ToolSource::Builtin,
            "a server cannot shadow a builtin name without permission"
        );
        assert_eq!(selection.diagnostics.len(), 1);
    }

    #[test]
    fn an_overridable_builtin_yields_its_name() {
        let policy = ToolPolicy {
            overridable: vec![ToolId {
                source: ToolSource::Builtin,
                local: "echo".into(),
            }],
            ..ToolPolicy::default()
        };
        let selection = policy.select(vec![
            RegisteredTool::builtin(Risk::Unknown, Arc::new(Echo)),
            RegisteredTool::new(
                ToolSource::Mcp("docs".into()),
                "echo",
                Risk::Unknown,
                Arc::new(Echo),
            ),
        ]);

        assert_eq!(selection.tools.len(), 1);
        assert_eq!(selection.tools[0].source(), &ToolSource::Mcp("docs".into()));
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
            async fn call(
                &self,
                _ctx: &ToolContext,
                _arguments: serde_json::Value,
            ) -> Result<ToolOutcome, ToolError> {
                Ok(ToolOutcome::success(""))
            }
        }

        let mut registry = registry();
        registry
            .register(RegisteredTool::builtin(Risk::Unknown, Arc::new(A)))
            .expect("unique name");

        let names: Vec<String> = registry.definitions().into_iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["aaa", "echo"]);
    }
}
