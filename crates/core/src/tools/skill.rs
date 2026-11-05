//! The `skill` tool: reads a skill's `SKILL.md` body on demand.
//!
//! The system instruction carries only the catalog — name, description and the
//! rule for resolving relative paths — so the body stays out of the context
//! until the model decides it needs it. Loading returns the instructions after
//! the frontmatter, framed with the skill's name and the file it came from.
//! The catalog's rule then applies to the body without repeating it.

use std::path::Path;

use serde::Deserialize;

use super::{failure_text, invalid_arguments, Tool, ToolContext, ToolError, ToolOutcome};
use crate::skills;

/// The name the model calls.
const NAME: &str = "skill";

/// The largest body handed back to the model, in bytes.
///
/// A skill is instructions, not a dataset. Truncating keeps one malformed or
/// unusually large file from crowding out the rest of the turn; the marker tells
/// the model what happened.
const MAX_BODY_BYTES: usize = 8_000;

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Load a skill's instructions by name. The system instruction lists the available \
skills; passing one of those names returns its instructions. Do this before \
following a skill, not after.";

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// The skill's name, as it appears in the catalog.
    name: String,
}

/// Loads a skill by name from the session's skill roots.
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
            Some(serde_json::Value::String(name)) if name.trim().is_empty() => {
                Err("/name: a skill name is required".to_owned())
            }
            Some(serde_json::Value::String(name)) if skills::is_valid_name(name.trim()) => Ok(()),
            Some(serde_json::Value::String(_)) => Err(
                "/name: use lowercase letters, digits, and single hyphens, at most 64 characters"
                    .to_owned(),
            ),
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
        let name = args.name.trim();

        let found = skills::discover(&skills::roots(&ctx.cwd));
        let Some(skill) = found.into_iter().find(|s| s.name == name) else {
            return Ok(ToolOutcome::failure(failure_text(
                format!("No skill named `{name}`."),
                Some("Load one of the names listed in the skills catalog."),
            )));
        };
        if !skill.model_invocable {
            return Ok(ToolOutcome::failure(failure_text(
                format!("Skill `{name}` is not model-invocable."),
                Some("Only skills listed in the catalog can be loaded by the model."),
            )));
        }

        match tokio::fs::read_to_string(&skill.path).await {
            Ok(text) => Ok(ToolOutcome::success(render(
                &skill.name,
                &skill.path,
                skills::body(&text),
            ))),
            Err(err) => Ok(ToolOutcome::failure(failure_text(
                format!("Cannot read {}: {err}", skill.path.display()),
                None,
            ))),
        }
    }
}

/// Frames the body with the skill's name and the file it came from.
fn render(name: &str, path: &Path, body: &str) -> String {
    let (body, truncated) = truncate_body(body);
    let note = if truncated {
        format!("\n\n[skill body truncated at {MAX_BODY_BYTES} bytes]")
    } else {
        String::new()
    };

    format!(
        "<skill name=\"{}\" location=\"{}\">\n{}{}\n</skill>",
        escape_attribute(name),
        escape_attribute(&path.display().to_string()),
        body.trim_end(),
        note
    )
}

/// Cuts a body at the last character boundary that fits the budget.
fn truncate_body(body: &str) -> (&str, bool) {
    if body.len() <= MAX_BODY_BYTES {
        return (body, false);
    }

    let mut end = MAX_BODY_BYTES;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    (&body[..end], true)
}

/// Escapes text placed in an attribute value.
fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(cwd: &Path) -> ToolContext {
        ToolContext {
            cwd: cwd.to_path_buf(),
            cancel: tokio_util::sync::CancellationToken::new(),
            sandbox: crate::sandbox::SandboxPolicy::Unrestricted,
        }
    }

    fn write_skill(
        root: &Path,
        name: &str,
        frontmatter: &str,
        body_text: &str,
    ) -> std::path::PathBuf {
        let dir = root.join(".srud").join("skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\n{frontmatter}---\n\n{body_text}"),
        )
        .unwrap();
        dir
    }

    #[tokio::test]
    async fn loads_the_named_skill_without_its_frontmatter() {
        let root = std::env::temp_dir().join(format!("srud-skill-tool-{}", std::process::id()));
        let dir = write_skill(&root, "demo", "name: demo\ndescription: D.\n", "Do it.\n");

        let outcome = SkillTool
            .call(&context(&root), serde_json::json!({ "name": "demo" }))
            .await
            .expect("registered");
        let dir_text = dir.to_string_lossy().into_owned();
        assert!(!outcome.is_error);
        assert!(outcome.output.contains("Do it."));
        assert!(outcome.output.contains(dir_text.as_str()));
        assert!(outcome.output.contains("<skill name=\"demo\" location="));
        assert!(!outcome.output.contains("description: D."));

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn an_unknown_name_is_a_failure_not_a_panic() {
        let root =
            std::env::temp_dir().join(format!("srud-skill-tool-empty-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();

        let outcome = SkillTool
            .call(&context(&root), serde_json::json!({ "name": "nope" }))
            .await
            .expect("registered");
        assert!(outcome.is_error);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_hidden_skill_is_not_loadable_by_the_model() {
        let root =
            std::env::temp_dir().join(format!("srud-skill-tool-hidden-{}", std::process::id()));
        write_skill(
            &root,
            "private",
            "name: private\ndescription: Hidden.\ndisable-model-invocation: true\n",
            "Secret.\n",
        );

        let outcome = SkillTool
            .call(&context(&root), serde_json::json!({ "name": "private" }))
            .await
            .expect("registered");
        assert!(outcome.is_error);
        assert!(outcome.output.contains("not model-invocable"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_frame_names_the_skill_and_file() {
        let rendered = render("x", Path::new("/a/b/SKILL.md"), "body");
        assert!(rendered.contains("<skill name=\"x\" location=\"/a/b/SKILL.md\">"));
        assert!(rendered.contains("\nbody\n</skill>"));
        assert!(!rendered.contains("References"));
    }

    #[test]
    fn a_long_body_is_truncated_with_a_marker() {
        let body = "a".repeat(MAX_BODY_BYTES + 100);
        let rendered = render("x", Path::new("/a/b/SKILL.md"), &body);
        assert!(rendered.contains("[skill body truncated at 8000 bytes]"));
        assert!(!rendered.contains(body.as_str()));
    }

    #[test]
    fn attribute_values_are_escaped() {
        let rendered = render("a&b\"c<d", Path::new("/a&b/SKILL.md"), "body");
        assert!(rendered.contains("name=\"a&amp;b&quot;c&lt;d\""));
        assert!(rendered.contains("location=\"/a&amp;b/SKILL.md\""));
    }

    #[test]
    fn validate_rejects_missing_or_invalid_names() {
        assert!(SkillTool.validate(&serde_json::json!({})).is_err());
        assert!(SkillTool
            .validate(&serde_json::json!({ "name": "  " }))
            .is_err());
        assert!(SkillTool
            .validate(&serde_json::json!({ "name": "Bad_Name" }))
            .is_err());
        assert!(SkillTool
            .validate(&serde_json::json!({ "name": "double--hyphen" }))
            .is_err());
        assert!(SkillTool
            .validate(&serde_json::json!({ "name": "pdf-tools" }))
            .is_ok());
    }

    #[test]
    fn the_tool_is_named_skill() {
        assert_eq!(SkillTool.name(), "skill");
    }
}
