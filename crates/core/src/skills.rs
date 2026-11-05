//! Skills: reusable instruction sets a model can pull in when a task calls for
//! them.
//!
//! A skill is a directory holding a `SKILL.md`. The file starts with a YAML
//! frontmatter block naming the skill, followed by the instructions. The turn
//! loop puts only each model-visible skill's name and description into the
//! system instruction; the body is read on demand through the `skill` tool.
//! Holding a pointer rather than the text keeps a large library of skills from
//! crowding the context, and lets a skill's relative paths resolve against its
//! own directory once the body is loaded.
//!
//! Frontmatter follows the Agent Skills specification: `name` and
//! `description` are required, `disable-model-invocation` hides a skill from the
//! model catalog, and `user-invocable` records whether a user-facing command
//! may invoke it. A skill that fails validation is skipped rather than allowed
//! to break the rest of the library.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

/// The file every skill is recognised by.
pub const SKILL_FILE: &str = "SKILL.md";

/// The longest accepted skill name, in characters.
pub const MAX_NAME_CHARS: usize = 64;

/// The longest accepted description, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 1024;

/// One discoverable skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The name the model uses to load it.
    pub name: String,
    /// One line on what it is for, shown in the catalog.
    pub description: String,
    /// The `SKILL.md` the body is read from.
    pub path: PathBuf,
    /// Whether the skill may appear in the model catalog and be loaded by it.
    pub model_invocable: bool,
    /// Whether a user-facing command may invoke the skill.
    pub user_invocable: bool,
    /// License name or bundled license file, when declared.
    pub license: Option<String>,
    /// Environment requirements, when declared.
    pub compatibility: Option<String>,
    /// Tools the skill declares as pre-approved; recorded, not enforced.
    pub allowed_tools: Vec<String>,
    /// Free-form metadata declared under the frontmatter `metadata` key.
    pub metadata: BTreeMap<String, serde_json::Value>,
}

/// The roots skills are collected from, nearest first.
///
/// Project skills live under `<cwd>/.srud/skills`; user-wide ones under
/// `$SRUD_HOME/skills`, falling back to `~/.srudagent/skills`. A project skill
/// shadows a user skill of the same name: the workspace is the more specific
/// statement of intent. An empty `cwd` has no project root, so callers that ask
/// before a workspace exists still see the user-wide library.
#[must_use]
pub fn roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if !cwd.as_os_str().is_empty() {
        roots.push(cwd.join(".srud").join("skills"));
    }
    if let Some(home) = srud_home() {
        roots.push(home.join("skills"));
    } else if let Some(user) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        roots.push(PathBuf::from(user).join(".srudagent").join("skills"));
    }
    roots
}

/// The user home from `SRUD_HOME`, when it is set to an absolute path.
///
/// The runtime has no handle on the server's configuration directory, so the
/// value is read from the environment rather than passed down.
fn srud_home() -> Option<PathBuf> {
    std::env::var_os("SRUD_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
}

/// Lists the skills under each root, nearest root winning name clashes.
///
/// Roots that do not exist are simply empty: a machine with no skills is the
/// common case, not an error. Entries that are not readable or do not pass
/// validation are skipped the same way — one broken skill must not hide the rest
/// of the library.
#[must_use]
pub fn discover(roots: &[PathBuf]) -> Vec<Skill> {
    let mut found: Vec<Skill> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let skill_file = dir.join(SKILL_FILE);
            if !dir.is_dir() || !skill_file.is_file() {
                continue;
            }
            if let Some(skill) = read_skill(&skill_file) {
                if !found.iter().any(|other| other.name == skill.name) {
                    found.push(skill);
                }
            }
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// Reads one skill from its `SKILL.md`, or `None` when it cannot be read or is
/// not valid.
fn read_skill(path: &Path) -> Option<Skill> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_skill(&text, path)
}

/// Parses one `SKILL.md` file, enforcing the parts of the Agent Skills
/// specification the runtime depends on.
fn parse_skill(text: &str, path: &Path) -> Option<Skill> {
    let (frontmatter, _) = split_frontmatter(text)?;
    let parsed: Frontmatter = serde_yaml::from_str(frontmatter).ok()?;

    let name = parsed.name?;
    if !is_valid_name(&name) {
        return None;
    }

    let description = normalize_description(&parsed.description?)?;

    Some(Skill {
        name,
        description,
        path: path.to_path_buf(),
        model_invocable: !parsed.disable_model_invocation.unwrap_or(false),
        user_invocable: parsed.user_invocable.unwrap_or(true),
        license: optional_text(parsed.license),
        compatibility: optional_text(parsed.compatibility),
        allowed_tools: parsed
            .allowed_tools
            .into_iter()
            .map(|tool| tool.trim().to_owned())
            .filter(|tool| !tool.is_empty())
            .collect(),
        metadata: parsed.metadata,
    })
}

/// Trims an optional text field and drops the ones that say nothing.
fn optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The frontmatter fields the runtime reads.
///
/// Unknown keys are ignored so other tools can keep their own frontmatter fields
/// in the same block.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default, rename = "disable-model-invocation")]
    disable_model_invocation: Option<bool>,
    #[serde(default, rename = "user-invocable")]
    user_invocable: Option<bool>,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(
        default,
        rename = "allowed-tools",
        deserialize_with = "deserialize_string_or_list"
    )]
    allowed_tools: Vec<String>,
    #[serde(default, rename = "metadata")]
    metadata: BTreeMap<String, serde_json::Value>,
}

/// Accepts `allowed-tools` as one string or a list, since frontmatter uses both
/// spellings.
fn deserialize_string_or_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    match Option::<OneOrMany>::deserialize(deserializer)? {
        None => Ok(Vec::new()),
        Some(OneOrMany::One(value)) => Ok(vec![value]),
        Some(OneOrMany::Many(values)) => Ok(values),
    }
}

/// Whether `name` matches the Agent Skills grammar: lowercase ASCII letters and
/// digits, single hyphens between them, at most [`MAX_NAME_CHARS`].
#[must_use]
pub fn is_valid_name(name: &str) -> bool {
    if name.chars().count() > MAX_NAME_CHARS {
        return false;
    }

    let mut previous_hyphen = false;
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            previous_hyphen = false;
        } else if ch == '-' && index > 0 && !previous_hyphen {
            previous_hyphen = true;
        } else {
            return false;
        }
    }

    !name.is_empty() && !previous_hyphen
}

/// Folds a description onto one line; returns `None` when it is empty or over
/// the catalog's limit.
fn normalize_description(raw: &str) -> Option<String> {
    let description = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() || description.chars().count() > MAX_DESCRIPTION_CHARS {
        return None;
    }
    Some(description)
}

/// Splits the leading `---` block from the body.
///
/// The opening fence must be the first line. A file without a closing fence is
/// not a skill: treating the rest as instructions would silently keep the
/// frontmatter in the prompt.
fn split_frontmatter(text: &str) -> Option<(&str, &str)> {
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;

    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == "---" {
            let frontmatter = &rest[..offset];
            let body_start = offset + line.len();
            return Some((frontmatter, &rest[body_start..]));
        }
        offset += line.len();
    }
    None
}

/// The body of a skill file: everything after the frontmatter block, or the
/// whole file when there is no block.
#[must_use]
pub fn body(text: &str) -> &str {
    split_frontmatter(text).map_or(text, |(_, body)| body.trim_start_matches(['\n', '\r']))
}

/// The catalog block placed in the system instruction.
///
/// One line per model-visible skill: name and description. The body and the
/// absolute path stay out — the `skill` tool resolves the path by name — so a
/// large library costs a line per skill rather than its instructions. The block
/// also carries the one rule for resolving relative paths in a loaded skill,
/// so each loaded body does not repeat it.
/// Empty when no model-visible skills were found, so the caller can drop the
/// section whole.
#[must_use]
pub fn render_catalog(skills: &[Skill]) -> String {
    let mut out = String::new();
    for skill in skills.iter().filter(|skill| skill.model_invocable) {
        if out.is_empty() {
            out.push_str("Available skills. Load one with the `skill` tool before following it.\n");
            out.push_str(
                "Relative paths mentioned in a skill resolve against the directory that contains its `SKILL.md`.\n",
            );
        }
        out.push_str(&format!("- {}: {}\n", skill.name, skill.description));
    }
    out
}

/// Lists the files bundled with a skill, relative to its directory and sorted.
///
/// `SKILL.md` is omitted — the caller already has it — and symbolic links are
/// skipped, so a link cycle cannot turn the listing into an infinite walk.
#[must_use]
pub fn supporting_files(path: &Path) -> Vec<String> {
    let Some(directory) = path.parent() else {
        return Vec::new();
    };

    let mut files = Vec::new();
    collect_supporting_files(directory, "", &mut files);
    files.sort();
    files
}

fn collect_supporting_files(directory: &Path, prefix: &str, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || matches!(name.as_str(), "node_modules") {
            continue;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };

        if file_type.is_dir() {
            collect_supporting_files(&entry.path(), &relative, files);
        } else if file_type.is_file() && name != SKILL_FILE {
            files.push(relative);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, dir: &str, frontmatter: &str, body_text: &str) {
        let dir = root.join(dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SKILL_FILE),
            format!("---\n{frontmatter}---\n\n{body_text}"),
        )
        .unwrap();
    }

    fn parsed(text: &str) -> Option<Skill> {
        parse_skill(text, Path::new("/x/SKILL.md"))
    }

    #[test]
    fn frontmatter_yields_name_and_description() {
        let skill = parsed("---\nname: pdf\ndescription: Handle PDFs.\n---\n\n# Body\n")
            .expect("valid skill");
        assert_eq!(skill.name, "pdf");
        assert_eq!(skill.description, "Handle PDFs.");
        assert!(skill.model_invocable);
        assert!(skill.user_invocable);
    }

    #[test]
    fn quoted_values_and_other_keys_are_tolerated() {
        let skill =
            parsed("---\nname: \"xlsx\"\nallowed-tools: Bash\ndescription: 'Sheets.'\n---\n")
                .expect("valid skill");
        assert_eq!(skill.name, "xlsx");
        assert_eq!(skill.description, "Sheets.");
    }

    #[test]
    fn a_block_description_is_folded_onto_one_line() {
        let skill =
            parsed("---\nname: pdf\ndescription: >\n  Read PDFs.\n  Convert them too.\n---\n")
                .expect("valid skill");
        assert_eq!(skill.description, "Read PDFs. Convert them too.");
    }

    #[test]
    fn standard_fields_are_read() {
        let skill = parsed(
            "---\nname: pdf\ndescription: D.\nlicense: MIT\ncompatibility: Linux\nallowed-tools: [Bash, Read]\nmetadata:\n  author: example\n  version: '1.2'\n---\n",
        )
        .expect("valid skill");
        assert_eq!(skill.license.as_deref(), Some("MIT"));
        assert_eq!(skill.compatibility.as_deref(), Some("Linux"));
        assert_eq!(skill.allowed_tools, vec!["Bash", "Read"]);
        assert_eq!(
            skill
                .metadata
                .get("author")
                .and_then(|value| value.as_str()),
            Some("example")
        );
        assert_eq!(
            skill
                .metadata
                .get("version")
                .and_then(|value| value.as_str()),
            Some("1.2")
        );
    }

    #[test]
    fn allowed_tools_accepts_one_string() {
        let skill =
            parsed("---\nname: pdf\ndescription: D.\nallowed-tools: Bash(git:*) Read\n---\n")
                .expect("valid skill");
        assert_eq!(skill.allowed_tools, vec!["Bash(git:*) Read"]);
    }

    #[test]
    fn visibility_flags_are_read() {
        let skill = parsed(
            "---\nname: private\ndescription: Hidden.\ndisable-model-invocation: true\nuser-invocable: false\n---\n",
        )
        .expect("valid skill");
        assert!(!skill.model_invocable);
        assert!(!skill.user_invocable);
    }

    #[test]
    fn no_frontmatter_is_not_a_skill() {
        assert!(parsed("# Just a heading\n").is_none());
    }

    #[test]
    fn a_missing_name_or_description_is_rejected() {
        assert!(parsed("---\ndescription: No name.\n---\n").is_none());
        assert!(parsed("---\nname: no-description\n---\n").is_none());
        assert!(parsed("---\nname: blank\ndescription: '  '\n---\n").is_none());
    }

    #[test]
    fn invalid_names_are_rejected() {
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("Upper"));
        assert!(!is_valid_name("with_underscore"));
        assert!(!is_valid_name("-leading"));
        assert!(!is_valid_name("trailing-"));
        assert!(!is_valid_name("double--hyphen"));
        assert!(!is_valid_name(&"a".repeat(MAX_NAME_CHARS + 1)));
        assert!(is_valid_name("pdf-tools-2"));
    }

    #[test]
    fn a_too_long_description_is_rejected() {
        let description = "a".repeat(MAX_DESCRIPTION_CHARS + 1);
        assert!(parsed(&format!(
            "---\nname: long\ndescription: {description}\n---\n"
        ))
        .is_none());
    }

    #[test]
    fn the_body_starts_after_the_frontmatter() {
        let text = "---\nname: x\ndescription: D.\n---\n\nDo the thing.\n";
        assert_eq!(body(text), "Do the thing.\n");
        assert_eq!(body("No fence at all.\n"), "No fence at all.\n");
    }

    #[test]
    fn discovery_finds_skills_and_sorts_them() {
        let root = std::env::temp_dir().join(format!("srud-skills-{}", std::process::id()));
        write_skill(&root, "beta", "name: beta\ndescription: B.\n", "b");
        write_skill(&root, "alpha", "name: alpha\ndescription: A.\n", "a");
        std::fs::create_dir_all(root.join("not-a-skill")).unwrap();

        let found = discover(std::slice::from_ref(&root));
        let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_root_is_empty_not_an_error() {
        let found = discover(&[PathBuf::from("/definitely/not/here")]);
        assert!(found.is_empty());
    }

    #[test]
    fn roots_without_a_workspace_are_user_only() {
        let found = roots(Path::new(""));
        assert!(found.len() <= 1);
        assert!(!found.iter().any(|root| root.starts_with(".srud")));
    }

    #[test]
    fn the_nearer_root_wins_a_name_clash() {
        let base = std::env::temp_dir().join(format!("srud-skills-shadow-{}", std::process::id()));
        let near = base.join("near");
        let far = base.join("far");
        write_skill(&near, "dup", "name: dup\ndescription: near.\n", "near");
        write_skill(&far, "dup", "name: dup\ndescription: far.\n", "far");

        let found = discover(&[near, far]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "near.");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_catalog_lists_model_visible_skills_without_paths() {
        let skills = vec![
            Skill {
                name: "pdf".into(),
                description: "Handle PDFs.".into(),
                path: PathBuf::from("/x/pdf/SKILL.md"),
                model_invocable: true,
                user_invocable: true,
                license: None,
                compatibility: None,
                allowed_tools: Vec::new(),
                metadata: BTreeMap::new(),
            },
            Skill {
                name: "private".into(),
                description: "Hidden.".into(),
                path: PathBuf::from("/x/private/SKILL.md"),
                model_invocable: false,
                user_invocable: true,
                license: None,
                compatibility: None,
                allowed_tools: Vec::new(),
                metadata: BTreeMap::new(),
            },
        ];
        let text = render_catalog(&skills);
        assert!(text.contains("- pdf: Handle PDFs."));
        assert!(text.contains(
            "Relative paths mentioned in a skill resolve against the directory that contains its `SKILL.md`."
        ));
        assert!(!text.contains("private"));
        assert!(!text.contains("/x/pdf/SKILL.md"));
        assert!(render_catalog(&[]).is_empty());
    }

    #[test]
    fn the_catalog_is_empty_when_no_skill_is_model_visible() {
        let skills = vec![Skill {
            name: "private".into(),
            description: "Hidden.".into(),
            path: PathBuf::from("/x/private/SKILL.md"),
            model_invocable: false,
            user_invocable: true,
            license: None,
            compatibility: None,
            allowed_tools: Vec::new(),
            metadata: BTreeMap::new(),
        }];
        assert!(render_catalog(&skills).is_empty());
    }

    #[test]
    fn metadata_is_parsed_into_the_skill() {
        let skill = parsed("---\nname: pdf\ndescription: D.\nmetadata:\n  author: example\n---\n")
            .expect("valid skill");
        assert_eq!(
            skill
                .metadata
                .get("author")
                .and_then(|value| value.as_str()),
            Some("example")
        );
    }

    #[test]
    fn supporting_files_are_relative_and_sorted() {
        let root = std::env::temp_dir().join(format!("srud-skill-files-{}", std::process::id()));
        write_skill(&root, "demo", "name: demo\ndescription: D.\n", "body");
        std::fs::create_dir_all(root.join("demo").join("scripts")).unwrap();
        std::fs::write(root.join("demo").join("scripts").join("run.sh"), "x").unwrap();
        std::fs::create_dir_all(root.join("demo").join("references")).unwrap();
        std::fs::write(root.join("demo").join("references").join("guide.md"), "x").unwrap();
        std::fs::write(root.join("demo").join("template.txt"), "x").unwrap();

        let files = supporting_files(&root.join("demo").join(SKILL_FILE));
        assert_eq!(
            files,
            vec!["references/guide.md", "scripts/run.sh", "template.txt"]
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
