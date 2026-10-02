//! The configuration directory.
//!
//! One directory holds everything the agent reads from the machine it runs on.
//! `SRUD_HOME` names it outright; otherwise it is `.srudagent` inside the user's
//! home. It is created at startup and is empty until something is put in it.

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
#[must_use]
pub fn home() -> Option<PathBuf> {
    env_path(HOME_VAR).or_else(|| {
        env_path(UNIX_HOME_VAR)
            .or_else(|| env_path(WINDOWS_HOME_VAR))
            .map(|user_home| user_home.join(DIR_NAME))
    })
}

/// The configuration directory, created if it is not there yet.
///
/// Creating it here rather than on first write keeps the "no settings" case and
/// the "settings exist" case on the same footing: the directory is always there,
/// so whatever writes into it does not have to create its own parent.
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use super::*;

    /// The variables are process-global, so these tests take a lock to keep
    /// from stepping on each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Keeps each temporary directory to itself.
    static NEXT_DIR: AtomicUsize = AtomicUsize::new(0);

    /// Puts the environment into a known state and returns a directory standing
    /// in for the user's home, so the default location is the one under test.
    fn with_user_home() -> (PathBuf, EnvGuard) {
        let guard = EnvGuard::take();
        std::env::remove_var(HOME_VAR);
        let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let user_home =
            std::env::temp_dir().join(format!("srud-config-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&user_home).expect("a temporary directory");
        std::env::set_var(UNIX_HOME_VAR, &user_home);
        (user_home, guard)
    }

    /// Restores the environment variables these tests touch.
    struct EnvGuard {
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn take() -> Self {
            let names = [HOME_VAR, UNIX_HOME_VAR, WINDOWS_HOME_VAR];
            let saved = names
                .iter()
                .map(|name| (*name, std::env::var(name).ok()))
                .collect();
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in self.saved.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn without_the_override_the_directory_is_under_the_user_home() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (user_home, _env) = with_user_home();

        assert_eq!(home(), Some(user_home.join(DIR_NAME)));
    }

    #[test]
    fn the_override_variable_names_the_directory_directly() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::take();
        let explicit = std::env::temp_dir().join("srud-config-explicit");
        std::env::set_var(HOME_VAR, &explicit);
        std::env::set_var(UNIX_HOME_VAR, "/home/someone");

        assert_eq!(
            home(),
            Some(explicit),
            "SRUD_HOME is the whole path, not a directory to create under the user home"
        );
    }

    #[test]
    fn a_blank_override_is_no_override() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::take();
        std::env::set_var(HOME_VAR, "   ");
        std::env::set_var(UNIX_HOME_VAR, "/home/someone");

        assert_eq!(
            home(),
            Some(PathBuf::from("/home/someone/.srudagent")),
            "an empty line behaves the same as omitting the variable"
        );
    }

    #[test]
    fn the_directory_is_created_when_it_is_missing() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let (user_home, _env) = with_user_home();

        let path = ensure().expect("the directory can be created");

        assert!(path.is_dir(), "{} is a directory", path.display());
        assert_eq!(path, user_home.join(DIR_NAME));
    }

    #[test]
    fn creating_it_twice_leaves_the_same_directory() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::take();
        let unique = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let nested = std::env::temp_dir()
            .join(format!(
                "srud-config-nested-{}-{unique}",
                std::process::id()
            ))
            .join("deeper");
        std::env::set_var(HOME_VAR, &nested);

        let path = ensure().expect("the whole path is created");

        assert_eq!(path, nested);
        assert!(nested.is_dir());
    }

    #[test]
    fn no_home_directory_at_all_is_reported() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let _env = EnvGuard::take();
        std::env::remove_var(HOME_VAR);
        std::env::remove_var(UNIX_HOME_VAR);
        std::env::remove_var(WINDOWS_HOME_VAR);

        assert_eq!(home(), None);
        assert!(matches!(ensure(), Err(ConfigError::NoHome)));
    }
}
