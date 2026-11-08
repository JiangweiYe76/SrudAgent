//! Sandbox policy vocabulary.
//!
//! The runtime has two ways to run a command: inside an OS boundary, or without
//! one. [`SandboxPolicy`] is that choice, and [`SandboxLevel`] is the user-facing
//! setting it is derived from.
//!
//! [`SandboxLevel::FullAccess`] maps to [`SandboxPolicy::Unrestricted`], which
//! spawns the caller's argv as written. The bounded levels map to
//! [`SandboxPolicy::Bounded`], which [`wrap_argv`] turns into a platform
//! runner's command through the [`SandboxBackend`] the build selects.
//! Network access is left as this machine has it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The mode a session runs in, as a user chooses it.
///
/// This is deliberately wider than [`SandboxMode`]: full access is a policy, not
/// an OS mode, and this enum is what a session remembers between turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxLevel {
    /// Reads are allowed; writes are refused.
    ReadOnly,
    /// Writes are limited to the workspace and its temporary directories.
    #[default]
    WorkspaceWrite,
    /// No OS boundary: the command runs with this process's own access.
    FullAccess,
}

impl SandboxLevel {
    /// The wire and log identifier.
    #[must_use]
    pub fn id(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::FullAccess => "full-access",
        }
    }

    /// The name a client shows.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::WorkspaceWrite => "Workspace write",
            Self::FullAccess => "Full access",
        }
    }

    /// What choosing this level means, in one sentence.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::ReadOnly => "The agent may read, but not write, any file this account can read.",
            Self::WorkspaceWrite => {
                "The agent may write inside the workspace and temporary directories only."
            }
            Self::FullAccess => {
                "The agent may read and write any file this account can, with no OS boundary."
            }
        }
    }

    /// Parses a level from the identifier used on the wire and in a log.
    #[must_use]
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "read-only" => Some(Self::ReadOnly),
            "workspace-write" => Some(Self::WorkspaceWrite),
            "full-access" => Some(Self::FullAccess),
            _ => None,
        }
    }

    /// The policy this level means for a session working in `cwd`.
    #[must_use]
    pub fn policy(self, cwd: &Path) -> SandboxPolicy {
        match self {
            Self::FullAccess => SandboxPolicy::Unrestricted,
            Self::ReadOnly => SandboxPolicy::Bounded(BoundedPolicy {
                mode: SandboxMode::ReadOnly,
                writable_roots: Vec::new(),
            }),
            Self::WorkspaceWrite => SandboxPolicy::Bounded(BoundedPolicy::workspace_write(cwd)),
        }
    }
}

/// A mode an OS boundary can express.
///
/// Full access is not one of these: there is no bounded policy that means
/// "unrestricted", so a full-access session cannot reach the wrapping path by
/// accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxMode {
    /// Reads only.
    ReadOnly,
    /// Writes inside the policy's roots only.
    WorkspaceWrite,
}

/// A policy that can be handed to the argv boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedPolicy {
    /// Which write shape applies.
    pub mode: SandboxMode,
    /// The only directories a workspace-write process may modify.
    ///
    /// Canonical, absolute, and de-duplicated, because this is the one source
    /// both the OS wrapper and the file tools are meant to agree on.
    pub writable_roots: Vec<PathBuf>,
}

impl BoundedPolicy {
    /// The policy for a workspace-write session working in `cwd`.
    #[must_use]
    pub fn workspace_write(cwd: &Path) -> Self {
        let mut roots = Vec::new();
        for candidate in [
            cwd.to_path_buf(),
            PathBuf::from("/tmp"),
            std::env::temp_dir(),
        ] {
            if let Some(root) = canonical_existing(&candidate) {
                if !roots.contains(&root) {
                    roots.push(root);
                }
            }
        }
        Self {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: roots,
        }
    }

    /// Whether a write to `path` is inside this policy.
    ///
    /// A path that does not exist yet is resolved through its nearest existing
    /// ancestor, so a write cannot escape by naming a directory that is created
    /// later in the same call.
    #[must_use]
    pub fn allows_write(&self, path: &Path) -> bool {
        if self.mode == SandboxMode::ReadOnly {
            return false;
        }
        let target = canonical_with_missing_tail(path);
        self.writable_roots
            .iter()
            .any(|root| target.starts_with(root))
    }
}

/// The policy a session runs under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxPolicy {
    /// A policy the argv boundary can wrap.
    Bounded(BoundedPolicy),
    /// No boundary: the caller spawns the raw argv.
    Unrestricted,
}

impl SandboxPolicy {
    /// Whether this policy runs without an OS boundary.
    #[must_use]
    pub fn is_unrestricted(&self) -> bool {
        matches!(self, Self::Unrestricted)
    }
}

/// How completely a policy was applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// Every boundary the policy names is in effect.
    Full,
    /// Some boundaries are missing; each is named.
    Partial {
        /// What could not be enforced.
        missing: Vec<String>,
    },
}

/// The argv to spawn, and what was enforced around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedArgv {
    /// The exact argv to hand to `Command`.
    pub argv: Vec<OsString>,
    /// What the wrapper actually enforced.
    pub enforcement: Enforcement,
}

mod platform;

pub use platform::Platform;

/// A bounded policy could not be turned into an executable argv.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxUnavailable {
    /// The runner a bounded policy needs is not on this machine.
    #[error("the sandbox runner `{program}` was not found")]
    RunnerMissing {
        /// The program that was looked for.
        program: &'static str,
    },
    /// The platform has no runner that implements this policy.
    #[error("sandboxing is not supported on {platform}")]
    UnsupportedPlatform {
        /// The platform the build targets.
        platform: Platform,
    },
}

/// The argv boundary a platform backend implements.
///
/// One implementation per platform runner, selected at compile time by the
/// [`wrap_argv`] dispatcher. The contract is the free function's contract:
/// return the argv that runs inside the boundary, or fail closed.
pub trait SandboxBackend {
    /// Wraps `argv` in the boundary `policy` describes.
    ///
    /// # Errors
    ///
    /// Returns [`SandboxUnavailable`] when the boundary cannot be prepared.
    /// The caller must fail closed, since running the raw argv here would
    /// silently turn a bounded request into full access.
    fn wrap_argv(
        &self,
        argv: &[OsString],
        policy: &BoundedPolicy,
    ) -> Result<WrappedArgv, SandboxUnavailable>;
}

/// Wraps `argv` in the boundary `policy` describes.
///
/// Dispatches to the [`SandboxBackend`] this build targets.
///
/// # Errors
///
/// Returns [`SandboxUnavailable::RunnerMissing`] or
/// [`SandboxUnavailable::UnsupportedPlatform`]. The caller must fail closed,
/// since running the raw argv here would silently turn a bounded request into
/// full access.
pub fn wrap_argv(
    argv: &[OsString],
    policy: &BoundedPolicy,
) -> Result<WrappedArgv, SandboxUnavailable> {
    platform::wrap(argv, policy)
}

/// Canonicalises an existing path, or returns `None` when it is not there.
fn canonical_existing(path: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

/// Canonicalises the nearest existing ancestor and reattaches the missing tail.
fn canonical_with_missing_tail(path: &Path) -> PathBuf {
    let mut tail = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match std::fs::canonicalize(&current) {
            Ok(mut resolved) => {
                for component in tail.iter().rev() {
                    resolved.push(component);
                }
                return resolved;
            }
            Err(_) => {
                let Some(parent) = current.parent() else {
                    return path.to_path_buf();
                };
                if let Some(name) = current.file_name() {
                    tail.push(name.to_os_string());
                }
                current = parent.to_path_buf();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("srud-sandbox-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    #[test]
    fn full_access_is_unrestricted() {
        let policy = SandboxLevel::FullAccess.policy(Path::new("/workspace"));
        assert!(policy.is_unrestricted());
    }

    #[test]
    fn workspace_write_is_default() {
        assert_eq!(SandboxLevel::default(), SandboxLevel::WorkspaceWrite);
    }

    #[test]
    fn an_unsupported_platform_names_itself() {
        let error = SandboxUnavailable::UnsupportedPlatform {
            platform: Platform::MacOs,
        };
        assert_eq!(error.to_string(), "sandboxing is not supported on macOS");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_roots_is_refused() {
        let dir = scratch("symlink");
        let link = dir.join("escape");
        std::os::unix::fs::symlink("/var/tmp", &link).expect("symlink");
        let policy = SandboxLevel::WorkspaceWrite.policy(&dir);
        let SandboxPolicy::Bounded(bounded) = policy else {
            panic!("workspace write is bounded");
        };
        assert!(
            !bounded.allows_write(&link.join("outside.txt")),
            "a symlink cannot carry a write past the roots"
        );
    }

    #[test]
    fn workspace_write_allows_inside_and_refuses_outside() {
        let dir = scratch("roots");
        let policy = SandboxLevel::WorkspaceWrite.policy(&dir);
        assert!(!policy.is_unrestricted());
        let SandboxPolicy::Bounded(bounded) = policy else {
            panic!("workspace write is bounded");
        };
        assert!(bounded.allows_write(&dir.join("inside.txt")));
        assert!(
            !bounded.allows_write(Path::new("/etc")),
            "a path outside every root is refused"
        );
    }

    #[test]
    fn read_only_refuses_every_write() {
        let dir = scratch("read-only");
        let policy = SandboxLevel::ReadOnly.policy(&dir);
        let SandboxPolicy::Bounded(bounded) = policy else {
            panic!("read only is bounded");
        };
        assert!(!bounded.allows_write(&dir.join("inside.txt")));
    }

    #[test]
    fn a_missing_tail_is_resolved_through_its_ancestor() {
        let dir = scratch("missing-tail");
        let policy = SandboxLevel::WorkspaceWrite.policy(&dir);
        let SandboxPolicy::Bounded(bounded) = policy else {
            panic!("workspace write is bounded");
        };
        assert!(bounded.allows_write(&dir.join("new/dir/file.txt")));
    }

    #[test]
    fn level_ids_round_trip() {
        for level in [
            SandboxLevel::ReadOnly,
            SandboxLevel::WorkspaceWrite,
            SandboxLevel::FullAccess,
        ] {
            assert_eq!(SandboxLevel::from_id(level.id()), Some(level));
        }
    }

    /// A backend used only to pin the trait's shape, independent of any runner.
    struct EchoBackend;

    impl SandboxBackend for EchoBackend {
        fn wrap_argv(
            &self,
            argv: &[OsString],
            _policy: &BoundedPolicy,
        ) -> Result<WrappedArgv, SandboxUnavailable> {
            Ok(WrappedArgv {
                argv: argv.to_vec(),
                enforcement: Enforcement::Full,
            })
        }
    }

    #[test]
    fn the_boundary_is_an_object_safe_trait() {
        let backend: &dyn SandboxBackend = &EchoBackend;
        let policy = BoundedPolicy {
            mode: SandboxMode::ReadOnly,
            writable_roots: Vec::new(),
        };
        let argv = vec![OsString::from("true")];
        let wrapped = backend
            .wrap_argv(&argv, &policy)
            .expect("echo backend wraps");
        assert_eq!(wrapped.argv, argv);
        assert_eq!(wrapped.enforcement, Enforcement::Full);
    }
}
