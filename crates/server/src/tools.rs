//! The tool set an agent starts with.
//!
//! Tools belong to the agent, not to whatever process hosts it: the turn loop
//! reads the registry, and the turn loop runs wherever this agent runs. A host
//! that assembled the tools would therefore hand the agent a capability set
//! that only exists in its own process — empty everywhere else.

use std::sync::Arc;

use srud_core::tools::{
    edit::EditTool, grep::GrepTool, read::ReadTool, shell::ShellTool, skill::SkillTool,
    write::WriteTool, Diagnostic, DuplicateTool, RegisteredTool, Risk, Tool, ToolPolicy,
    ToolRegistry,
};

/// The builtin tool set, kept apart so a session can assemble it with other
/// sources.
pub struct BuiltinSource;

impl BuiltinSource {
    /// The tools this source offers, with their risk levels.
    #[must_use]
    pub fn tools(&self) -> Vec<RegisteredTool> {
        let mut tools = Vec::new();
        add(&mut tools, Risk::ReadOnly, ReadTool);
        add(&mut tools, Risk::Writes, EditTool);
        add(&mut tools, Risk::Writes, WriteTool);
        add(&mut tools, Risk::Executes, ShellTool::default());
        add(&mut tools, Risk::ReadOnly, GrepTool);
        add(&mut tools, Risk::ReadOnly, SkillTool);
        tools
    }
}

/// Adds one builtin under the name it gives itself.
fn add(tools: &mut Vec<RegisteredTool>, risk: Risk, tool: impl Tool + 'static) {
    tools.push(RegisteredTool::builtin(risk, Arc::new(tool)));
}

/// The tools every agent is built with.
///
/// The set is small on purpose: it is the whole of what the model can reach, and
/// every entry here is a capability this crate carries into each turn.
#[must_use]
pub fn standard_tools() -> Arc<ToolRegistry> {
    Arc::new(ToolRegistry::from_tools(BuiltinSource.tools()).expect("the builtin names are unique"))
}

/// Rebuilds a session's registry from its base set and what another source added.
///
/// The policy resolves name clashes and drops denied tools; the diagnostics say
/// what it left out.
///
/// # Errors
///
/// Returns [`DuplicateTool`] if the policy let two tools keep one name, which
/// would be a bug in the policy rather than in the caller.
pub fn rebuild(
    base: &ToolRegistry,
    extra: Vec<RegisteredTool>,
    policy: &ToolPolicy,
) -> Result<(Arc<ToolRegistry>, Vec<Diagnostic>), DuplicateTool> {
    let mut candidates = base.entries();
    candidates.extend(extra);
    let selection = policy.select(candidates);
    let registry = Arc::new(ToolRegistry::from_tools(selection.tools)?);
    Ok((registry, selection.diagnostics))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_set_can_read_edit_write_search_and_run() {
        let tools = standard_tools();
        assert!(tools.contains("read"), "the read tool is registered");
        assert!(tools.contains("edit"), "the edit tool is registered");
        assert!(tools.contains("write"), "the write tool is registered");
        assert!(tools.contains("shell"), "the shell tool is registered");
        assert!(tools.contains("grep"), "the grep tool is registered");
        assert!(tools.contains("skill"), "the skill tool is registered");
    }

    #[test]
    fn each_agent_gets_a_registry_of_its_own() {
        // Two agents must not share one registry: a tool registered on one
        // would silently appear in what the other offers the model.
        assert!(!Arc::ptr_eq(&standard_tools(), &standard_tools()));
    }

    #[test]
    fn every_standard_tool_is_described_and_takes_an_object() {
        for definition in standard_tools().definitions() {
            assert!(
                !definition.description.trim().is_empty(),
                "{} has no description",
                definition.name
            );
            assert_eq!(
                definition.parameters["type"], "object",
                "{} takes an arguments object",
                definition.name
            );
        }
    }

    #[test]
    fn a_denied_tool_is_left_out_of_the_session() {
        use srud_core::tools::{ToolId, ToolSource};

        let policy = ToolPolicy {
            deny: vec![ToolId {
                source: ToolSource::Builtin,
                local: "shell".into(),
            }],
            ..ToolPolicy::default()
        };

        let (tools, diagnostics) =
            rebuild(&standard_tools(), Vec::new(), &policy).expect("unique names");

        assert!(tools.contains("read"));
        assert!(!tools.contains("shell"));
        assert_eq!(diagnostics.len(), 1);
    }
}
