//! The tool set an agent starts with.
//!
//! Tools belong to the agent, not to whatever process hosts it: the turn loop
//! reads the registry, and the turn loop runs wherever this agent runs. A host
//! that assembled the tools would therefore hand the agent a capability set
//! that only exists in its own process — empty everywhere else.

use std::sync::Arc;

use srud_core::tools::{
    bash::BashTool, edit::EditTool, read::ReadTool, write::WriteTool, ToolRegistry,
};

/// The tools every agent is built with.
///
/// The set is small on purpose: it is the whole of what the model can reach, and
/// every entry here is a capability this crate carries into each turn.
#[must_use]
pub fn standard_tools() -> Arc<ToolRegistry> {
    let mut tools = ToolRegistry::new();
    tools
        .register(Arc::new(ReadTool))
        .expect("the registry is fresh");
    tools
        .register(Arc::new(EditTool))
        .expect("the registry is fresh");
    tools
        .register(Arc::new(WriteTool))
        .expect("the registry is fresh");
    tools
        .register(Arc::new(BashTool::default()))
        .expect("the registry is fresh");
    Arc::new(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_standard_set_can_read_edit_write_and_run() {
        let tools = standard_tools();
        assert!(tools.contains("read"), "the read tool is registered");
        assert!(tools.contains("edit"), "the edit tool is registered");
        assert!(tools.contains("write"), "the write tool is registered");
        assert!(tools.contains("bash"), "the bash tool is registered");
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
}
