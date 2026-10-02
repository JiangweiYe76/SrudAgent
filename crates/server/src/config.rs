//! The configuration directory.
//!
//! One directory holds everything the agent reads from and writes to on the
//! machine it runs on. `SRUD_HOME` names it outright; otherwise it is
//! `.srudagent` inside the user's home. It is created at startup, and holds a
//! `workspaces/` directory of one workspace per session.

use std::path::PathBuf;

/// Names the configuration directory outright, overriding the default.
pub const HOME_VAR: &str = "SRUD_HOME";

/// Holds the user's home directory on unix.
const UNIX_HOME_VAR: &str = "HOME";

/// Holds the user's home directory on Windows.
const WINDOWS_HOME_VAR: &str = "USERPROFILE";

/// The directory created under the user's home when [`HOME_VAR`] is unset.
const DIR_NAME: &str = ".srudagent";

/// Why the configuration directory could not be established.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Neither the override nor the user's home directory could be found.
    #[error(
        "cannot locate the configuration directory: SRUD_HOME, HOME and USERPROFILE are all unset"
    )]
    NoHome,

    /// The directory could not be created.
    #[error("cannot create {path}: {source}")]
    Create {
        /// The directory that could not be created.
        path: PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },
}

/// The configuration directory, whether or not it exists yet.
///
/// `SRUD_HOME` wins; otherwise `.srudagent` under the user's home directory. A
/// blank variable counts as unset, so an empty line in a dotenv file behaves the
/// same as omitting it.
///
/// A `SRUD_HOME` that is not an absolute path counts as unset too. Everything
/// under this directory — a session's workspace above all — is created and later
/// removed by path, and a relative path names a different place depending on
/// where the agent happened to be started. Pointing it at `.` would quietly put
/// a `workspaces/` directory inside whatever directory that turned out to be,
/// which is as likely to be someone's project as anything else.
#[must_use]
pub fn home() -> Option<PathBuf> {
    absolute_path(HOME_VAR).or_else(|| user_home().map(|user_home| user_home.join(DIR_NAME)))
}

/// The user's home directory, whichever variable names it on this platform.
///
/// A value that is not an absolute path counts as unset, for the same reason
/// [`home`] gives.
#[must_use]
pub fn user_home() -> Option<PathBuf> {
    absolute_path(UNIX_HOME_VAR).or_else(|| absolute_path(WINDOWS_HOME_VAR))
}

/// The configuration directory, created if it is not there yet.
///
/// Creating it here rather than on first write means a directory that cannot be
/// written to is reported at startup, instead of surfacing later as whatever
/// write happened to fail.
///
/// # Errors
///
/// Returns [`ConfigError::NoHome`] when there is no home directory to put it
/// under, and [`ConfigError::Create`] when the filesystem refuses.
pub fn ensure() -> Result<PathBuf, ConfigError> {
    let path = home().ok_or(ConfigError::NoHome)?;
    std::fs::create_dir_all(&path).map_err(|source| ConfigError::Create {
        path: path.clone(),
        source,
    })?;
    Ok(path)
}

/// Reads a variable as a path, treating blank as unset.
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Reads a variable as a path, treating blank and relative as unset.
///
/// A relative path is worse than no setting at all: it resolves against
/// whichever directory the process was started in, so the same variable names a
/// different directory on two runs.
fn absolute_path(name: &str) -> Option<PathBuf> {
    env_path(name).filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use std::env;

    use crate::test_env::{self, Guard};

    use super::*;

    /// The variables that decide where the configuration directory lives.
    const NAMES: &[&str] = &[HOME_VAR, UNIX_HOME_VAR, WINDOWS_HOME_VAR];

    /// Puts the environment into a known state and returns a directory standing
    /// in for the user's home, so the default location is the one under test.
    fn with_user_home() -> (PathBuf, Guard) {
        let guard = Guard::take(NAMES);
        env::remove_var(HOME_VAR);
        let user_home = test_env::unique_dir("srud-config");
        env::set_var(UNIX_HOME_VAR, &user_home);
        (user_home, guard)
    }

    #[test]
    fn a_relative_override_is_no_override() {
        let (user_home, _env) = with_user_home();
        env::set_var(HOME_VAR, ".");

        assert_eq!(
            home(),
            Some(user_home.join(DIR_NAME)),
            "a relative path would put the directory wherever the agent was started"
        );
    }

    #[test]
    fn a_relative_override_leaves_the_directory_under_the_user_home() {
        let (user_home, _env) = with_user_home();
        env::set_var(HOME_VAR, "./somewhere");

        let path = ensure().expect("the directory can be created");

        assert!(
            path.starts_with(&user_home),
            "{} is under {}, not under wherever the agent was started",
            path.display(),
            user_home.display()
        );
        assert!(path.is_absolute());
    }

    #[test]
    fn a_relative_user_home_counts_as_no_user_home() {
        let _env = Guard::take(NAMES);
        for name in NAMES {
            env::remove_var(name);
        }
        env::set_var(UNIX_HOME_VAR, "relative/home");

        assert_eq!(user_home(), None);
        assert_eq!(home(), None);
    }

    #[test]
    fn a_blank_user_home_counts_as_no_user_home() {
        let _env = Guard::take(NAMES);
        for name in NAMES {
            env::remove_var(name);
        }
        env::set_var(UNIX_HOME_VAR, "  ");

        assert_eq!(user_home(), None, "an empty line is not a path");
        assert_eq!(home(), None);
    }

    #[test]
    fn without_the_override_the_directory_is_under_the_user_home() {
        let (user_home, _env) = with_user_home();

        assert_eq!(home(), Some(user_home.join(DIR_NAME)));
    }

    #[test]
    fn the_override_variable_names_the_directory_directly() {
        let _env = Guard::take(NAMES);
        let explicit = env::temp_dir().join("srud-config-explicit");
        env::set_var(HOME_VAR, &explicit);
        env::set_var(UNIX_HOME_VAR, "/home/someone");

        assert_eq!(
            home(),
            Some(explicit),
            "SRUD_HOME is the whole path, not a directory to create under the user home"
        );
    }

    #[test]
    fn a_blank_override_is_no_override() {
        let _env = Guard::take(NAMES);
        env::set_var(HOME_VAR, "   ");
        env::set_var(UNIX_HOME_VAR, "/home/someone");

        assert_eq!(
            home(),
            Some(PathBuf::from("/home/someone/.srudagent")),
            "an empty line behaves the same as omitting the variable"
        );
    }

    #[test]
    fn the_directory_is_created_when_it_is_missing() {
        let (user_home, _env) = with_user_home();

        let path = ensure().expect("the directory can be created");

        assert!(path.is_dir(), "{} is a directory", path.display());
        assert_eq!(path, user_home.join(DIR_NAME));
    }

    #[test]
    fn creating_it_twice_leaves_the_same_directory() {
        let (_user_home, _env) = with_user_home();
        // A marker stands in for whatever the directory holds once it is used.
        std::fs::write(ensure().expect("created").join("marker"), "kept").expect("marker");

        let second = ensure().expect("created again");

        assert_eq!(
            std::fs::read_to_string(second.join("marker")).expect("survived"),
            "kept",
            "an existing directory is left as it is"
        );
    }

    #[test]
    fn the_override_is_created_even_where_nothing_exists_yet() {
        let _env = Guard::take(NAMES);
        let nested = test_env::unique_dir("srud-config-nested").join("deeper");
        env::set_var(HOME_VAR, &nested);

        let path = ensure().expect("the whole path is created");

        assert_eq!(path, nested);
        assert!(nested.is_dir());
    }

    #[test]
    fn no_home_directory_at_all_is_reported() {
        let _env = Guard::take(NAMES);
        for name in NAMES {
            env::remove_var(name);
        }

        assert_eq!(home(), None);
        assert!(matches!(ensure(), Err(ConfigError::NoHome)));
    }
}
