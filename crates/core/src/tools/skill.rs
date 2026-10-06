//! The `skill` tool: reads a skill's `SKILL.md` body on demand.
//!
//! The system instruction carries only the catalog — name, description, path —
//! so the body stays out of the context until the model decides it needs it.
//! Loading means reading the whole file, frontmatter included: the model is
//! told how the skill frames its own work, and the frontmatter is a line or two.

use std::path::Path;

use serde::Deserialize;

use super::{failure_text, invalid_arguments, Tool, ToolContext, ToolError, ToolOutcome};
use crate::skills;

/// The name the model calls.
const NAME: &str = "skill";

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Load a skill by name. The system instruction lists the available skills; \
passing one of those names returns its instructions. Do this before following \
a skill, not after.";

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// The skill's name, as it appears in the catalog.
    name: String,
}

/// Loads skills from the session's working directory.
pub struct SkillTool;

#[async_trait::async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill's name, as it appears in the catalog."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    fn validate(&self, arguments: &serde_json::Value) -> Result<(), String> {
        match arguments.get("name") {
            Some(serde_json::Value::String(name)) if !name.trim().is_empty() => Ok(()),
            _ => Err("/name: a skill name is required".to_owned()),
        }
    }

    async fn call(
        &self,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let args: Args = serde_json::from_value(arguments)
            .map_err(|err| invalid_arguments(NAME, err.to_string()))?;

        let found = skills::discover(&skills::roots(&ctx.cwd));
        let Some(skill) = found.into_iter().find(|s| s.name == args.name) else {
            return Ok(ToolOutcome::failure(failure_text(
                format!("No skill named `{}`.", args.name),
                Some("Load one of the names listed in the skills catalog."),
            )));
        };

        match tokio::fs::read_to_string(&skill.path).await {
            Ok(text) => Ok(ToolOutcome::success(render(
                &skill.name,
                &skill.path,
                &text,
            ))),
            Err(err) => Ok(ToolOutcome::failure(failure_text(
                format!("Cannot read {}: {err}", skill.path.display()),
                None,
            ))),
        }
    }
}

/// Frames the body so the model knows which directory its relative paths run
/// from.
fn render(name: &str, path: &Path, text: &str) -> String {
    let dir = path.parent().unwrap_or(path);
    format!(
        "<skill name=\"{name}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        dir.display(),
        text.trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_the_named_skill() {
        let root = std::env::temp_dir().join(format!("srud-skill-tool-{}", std::process::id()));
        let dir = root.join(".srud").join("skills").join("demo");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: demo\ndescription: D.\n---\n\nDo it.\n",
        )
        .unwrap();

        let ctx = ToolContext { cwd: root.clone() };
        let outcome = SkillTool
            .call(&ctx, serde_json::json!({ "name": "demo" }))
            .await
            .expect("registered");
        assert!(!outcome.is_error);
        assert!(outcome.output.contains("Do it."));
        assert!(outcome.output.contains(dir.to_string_lossy().as_ref()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn an_unknown_name_is_a_failure_not_a_panic() {
        let root =
            std::env::temp_dir().join(format!("srud-skill-tool-empty-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ToolContext { cwd: root.clone() };
        let outcome = SkillTool
            .call(&ctx, serde_json::json!({ "name": "nope" }))
            .await
            .expect("registered");
        assert!(outcome.is_error);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_path_note_anchors_relative_paths() {
        let rendered = render("x", Path::new("/a/b/SKILL.md"), "body");
        assert!(rendered.contains("References are relative to /a/b."));
        assert!(rendered.contains("<skill name=\"x\">"));
    }

    #[test]
    fn validate_rejects_a_missing_name() {
        assert!(SkillTool.validate(&serde_json::json!({})).is_err());
        assert!(SkillTool
            .validate(&serde_json::json!({ "name": "  " }))
            .is_err());
        assert!(SkillTool
            .validate(&serde_json::json!({ "name": "ok" }))
            .is_ok());
    }

    #[test]
    fn the_tool_is_named_skill() {
        assert_eq!(SkillTool.name(), "skill");
    }
}
