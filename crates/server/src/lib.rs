//! The ACP v1 agent service for SrudAgent.
//!
//! This crate turns the core runtime into a protocol-compliant ACP agent:
//! it dispatches JSON-RPC requests, drives one [`run_turn`] per
//! `session/prompt`, and republishes the runtime's events as
//! `session/update` notifications.
//!
//! It is **transport-agnostic**: it listens on nothing. A host (the Tauri
//! shell, or a test) feeds it [`RpcRequest`] envelopes through
//! [`Agent::handle`] and receives the JSON-RPC reply, while
//! [`Agent::subscribe`] yields the outgoing notification stream.
//!
//! Behavioural boundaries:
//!
//! - In-memory sessions only — no rollout store, so `session/load`,
//!   `session/resume`, and the `_srud/unstable/*` extensions answer
//!   `METHOD_NOT_FOUND` and are not advertised in the capabilities.
//! - No approval flow — `session/request_permission` is not implemented; a tool
//!   call runs unless the tool itself refuses it. `bash` refuses a list of
//!   commands by name, which is not a boundary: a command that deletes a file
//!   without naming a deleted program goes through.
//! - Text-only prompts — non-text content blocks are rejected with
//!   `INVALID_PARAMS`, matching the declared prompt capabilities.
//!
//! What a host supplies and what it does not:
//!
//! - The model client — [`Agent::new`] takes one, so a test can hand over a
//!   scripted model and a live host can pick a provider.
//! - Nothing else. The tool set comes from [`standard_tools`], because the
//!   tools are read by the turn loop, which runs here rather than in the host.

mod agent;
pub mod config;
mod convert;
mod events;
mod sessions;
mod tools;

pub use agent::{Agent, RpcReply};
pub use sessions::{CreateError, SessionManager};
pub use tools::standard_tools;

/// Test support for the process-global environment.
///
/// Variables such as `HOME` are shared by every test in the binary, so a module
/// that saves them takes one lock for the length of its test: two locks in two
/// modules would let them save and restore over each other.
#[cfg(test)]
pub(crate) mod test_env {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    /// Blocks until this thread owns the environment.
    fn lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Holds the environment for one test, restoring the named variables when it
    /// is dropped.
    pub struct Guard {
        _lock: MutexGuard<'static, ()>,
        saved: Vec<(&'static str, Option<String>)>,
    }

    impl Guard {
        /// Takes the environment and remembers the current value of each name.
        pub fn take(names: &[&'static str]) -> Self {
            let lock = lock();
            let saved = names
                .iter()
                .map(|name| (*name, env::var(name).ok()))
                .collect();
            Self { _lock: lock, saved }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            for (name, value) in self.saved.drain(..) {
                match value {
                    Some(value) => env::set_var(name, value),
                    None => env::remove_var(name),
                }
            }
        }
    }

    /// An empty directory of its own under the system temp directory.
    pub fn unique_dir(prefix: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!("{prefix}-{}-{unique}", process::id()));
        fs::create_dir_all(&path).expect("a temporary directory");
        path
    }
}
