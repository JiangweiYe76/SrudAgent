//! Skills: reusable instruction sets a model can pull in when a task calls for
//! them.
//!
//! A skill is a directory holding a `SKILL.md`. The file starts with a
//! frontmatter block naming the skill, followed by the instructions. The turn
//! loop puts only each skill's name, description and path into the system
//! instruction; the body is read on demand through the `skill` tool. Holding a
//! pointer rather than the text keeps a large library of skills from crowding
//! the context, and lets a skill's relative paths resolve against its own
//! directory once the body is loaded.

use std::path::{Path, PathBuf};

/// The file every skill is recognised by.
pub const SKILL_FILE: &str = "SKILL.md";

/// One discoverable skill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    /// The name the model uses to load it.
    pub name: String,
    /// One line on what it is for, shown in the catalog.
    pub description: String,
    /// The `SKILL.md` the body is read from.
    pub path: PathBuf,
}

/// The roots skills are collected from, nearest first.
///
/// Project skills live under `<cwd>/.srud/skills`; user-wide ones under
/// `$SRUD_HOME/skills`, falling back to `~/.srudagent/skills`. A project skill
/// shadows a user skill of the same name: the workspace is the more specific
/// statement of intent.
#[must_use]
pub fn roots(cwd: &Path) -> Vec<PathBuf> {
    let mut roots = vec![cwd.join(".srud").join("skills")];
    if let Some(home) = srud_home() {
        roots.push(home.join("skills"));
    } else if let Some(user) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        roots.push(PathBuf::from(user).join(".srudagent").join("skills"));
    }
    roots
}

/// The turn loop reads the variable itself because it has no handle on the
/// server's configuration directory, and the catalog would otherwise have to
/// be threaded through every `run_turn` call.
fn srud_home() -> Option<PathBuf> {
    std::env::var_os("SRUD_HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty() && path.is_absolute())
}

/// Lists the skills under each root, nearest root winning name clashes.
///
/// Roots that do not exist are simply empty: a machine with no skills is the
/// common case, not an error. Entries that are not readable are skipped the
/// same way — one broken skill must not hide the rest of the library.
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

/// Reads one skill's name and description from its `SKILL.md`.
fn read_skill(path: &Path) -> Option<Skill> {
    let text = std::fs::read_to_string(path).ok()?;
    let (name, description) = parse_frontmatter(&text);
    let name = name.or_else(|| path.parent()?.file_name()?.to_str().map(str::to_owned))?;
    Some(Skill {
        name,
        description: description.unwrap_or_default(),
        path: path.to_path_buf(),
    })
}

/// Pulls `name` and `description` out of a leading frontmatter block.
///
/// The block is the `---`-fenced list of `key: value` lines at the top of the
/// file. Only those two keys matter here; anything else is ignored, so fields
/// a richer runtime understands simply pass through. A value may be bare or
/// quoted, since hand-written frontmatter varies.
#[must_use]
pub fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return (None, None);
    };
    let mut name = None;
    let mut description = None;
    for line in rest.lines() {
        if line.trim() == "---" {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            match key.trim() {
                "name" => name = Some(value.to_owned()),
                "description" => description = Some(value.to_owned()),
                _ => {}
            }
        }
    }
    (name, description)
}

/// The body of a skill file: everything after the frontmatter block.
#[must_use]
pub fn body(text: &str) -> &str {
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return text;
    };
    match rest.find("\n---") {
        Some(end) => {
            let after = &rest[end + 4..];
            after.trim_start_matches(['\n', '\r'])
        }
        None => text,
    }
}

/// The catalog block placed in the system instruction.
///
/// One line per skill: name, description, and where the body lives. Empty when
/// no skills were found, so the caller can drop the section whole.
#[must_use]
pub fn render_catalog(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out =
        String::from("Available skills. Load one with the `skill` tool before following it.\n");
    for skill in skills {
        if skill.description.is_empty() {
            out.push_str(&format!("- {} ({})\n", skill.name, skill.path.display()));
        } else {
            out.push_str(&format!(
                "- {}: {} ({})\n",
                skill.name,
                skill.description,
                skill.path.display()
            ));
        }
    }
    out
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

    #[test]
    fn frontmatter_yields_name_and_description() {
        let text = "---\nname: pdf\ndescription: Handle PDFs.\n---\n\n# Body\n";
        let (name, description) = parse_frontmatter(text);
        assert_eq!(name.as_deref(), Some("pdf"));
        assert_eq!(description.as_deref(), Some("Handle PDFs."));
    }

    #[test]
    fn quoted_values_and_other_keys_are_tolerated() {
        let text = "---\nname: \"xlsx\"\nallowed-tools: Bash\ndescription: 'Sheets.'\n---\n";
        let (name, description) = parse_frontmatter(text);
        assert_eq!(name.as_deref(), Some("xlsx"));
        assert_eq!(description.as_deref(), Some("Sheets."));
    }

    #[test]
    fn no_frontmatter_is_not_an_error() {
        let (name, description) = parse_frontmatter("# Just a heading\n");
        assert_eq!(name, None);
        assert_eq!(description, None);
    }

    #[test]
    fn the_body_starts_after_the_frontmatter() {
        let text = "---\nname: x\n---\n\nDo the thing.\n";
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
    fn the_catalog_lists_each_skill_once() {
        let skills = vec![Skill {
            name: "pdf".into(),
            description: "Handle PDFs.".into(),
            path: PathBuf::from("/x/pdf/SKILL.md"),
        }];
        let text = render_catalog(&skills);
        assert!(text.contains("- pdf: Handle PDFs. (/x/pdf/SKILL.md)"));
        assert!(render_catalog(&[]).is_empty());
    }
}
