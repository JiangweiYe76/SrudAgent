//! The tool policy file.
//!
//! One file holds the policy applied to every session's tool set: what to deny
//! and which builtin names a lower-precedence source may replace. It lives under
//! the configuration directory as `tools.json`, beside the provider and MCP
//! files, because it is the user's to edit.
//!
//! ```json
//! {
//!   "deny": ["builtin/shell"],
//!   "overridable": ["builtin/read"]
//! }
//! ```

use std::path::PathBuf;

use srud_core::tools::ToolPolicy;

use crate::config;

/// Why the tool policy file could not be used.
#[derive(Debug, thiserror::Error)]
pub enum ToolPolicyError {
    /// There is no configuration directory to put the file in.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// The file could not be read.
    #[error("Cannot use {path}: {source}")]
    Io {
        /// The file that could not be read.
        path: PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },

    /// The file does not hold a policy this build understands.
    #[error("{path} is not a tool policy this build can read: {message}")]
    Parse {
        /// The file that could not be read.
        path: PathBuf,
        /// What was wrong with it.
        message: String,
    },
}

/// Reads the tool policy file, or returns the default when there is none.
///
/// # Errors
///
/// [`ToolPolicyError::Config`] when there is no configuration directory to look
/// in, [`ToolPolicyError::Io`] when the file cannot be read, and
/// [`ToolPolicyError::Parse`] when it holds something this build cannot
/// interpret. A missing file is not an error: it means nothing is restricted.
pub fn load() -> Result<ToolPolicy, ToolPolicyError> {
    load_from(config::tools_path()?)
}

/// Reads a policy from a specific file.
///
/// For a caller that names the file itself rather than taking the default: a
/// test, or a host with its own configuration source.
///
/// # Errors
///
/// As [`load`].
pub fn load_from(path: impl Into<PathBuf>) -> Result<ToolPolicy, ToolPolicyError> {
    let path = path.into();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ToolPolicy::default())
        }
        Err(source) => return Err(ToolPolicyError::Io { path, source }),
    };

    serde_json::from_str(&text).map_err(|error| ToolPolicyError::Parse {
        path,
        message: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env;

    fn written(name: &str, text: &str) -> PathBuf {
        let dir = test_env::unique_dir("srud-tools");
        std::fs::create_dir_all(&dir).expect("a directory");
        let path = dir.join(name);
        std::fs::write(&path, text).expect("a file");
        path
    }

    #[test]
    fn a_missing_file_keeps_every_tool() {
        let dir = test_env::unique_dir("srud-tools-missing");

        let policy = load_from(dir.join("tools.json")).expect("a missing file is empty");

        assert!(policy.deny.is_empty());
        assert!(policy.overridable.is_empty());
    }

    #[test]
    fn a_hand_written_file_is_read() {
        let path = written(
            "tools.json",
            r#"{"deny":["builtin/shell"],"overridable":["builtin/read"]}"#,
        );

        let policy = load_from(&path).expect("a valid policy");

        assert_eq!(policy.deny[0].to_string(), "builtin/shell");
        assert_eq!(policy.overridable[0].to_string(), "builtin/read");
    }

    #[test]
    fn an_unknown_key_is_refused() {
        let path = written("bad.json", r#"{"denny":["builtin/shell"]}"#);

        assert!(matches!(
            load_from(&path),
            Err(ToolPolicyError::Parse { .. })
        ));
    }

    #[test]
    fn a_tool_id_the_build_cannot_parse_is_refused() {
        let path = written("bad-id.json", r#"{"deny":["shell"]}"#);

        assert!(matches!(
            load_from(&path),
            Err(ToolPolicyError::Parse { .. })
        ));
    }
}
