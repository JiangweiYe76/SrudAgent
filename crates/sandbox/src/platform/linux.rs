//! The Linux backend: a bubblewrap mount view over the caller's argv.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::{BoundedPolicy, Enforcement, SandboxBackend, SandboxUnavailable, WrappedArgv};

/// The bubblewrap backend.
pub(super) struct Bwrap;

impl SandboxBackend for Bwrap {
    fn wrap_argv(
        &self,
        argv: &[OsString],
        policy: &BoundedPolicy,
    ) -> Result<WrappedArgv, SandboxUnavailable> {
        let runner = find_bwrap().ok_or(SandboxUnavailable::RunnerMissing { program: "bwrap" })?;
        let mut wrapped = Vec::with_capacity(argv.len() + policy.writable_roots.len() * 3 + 16);
        wrapped.push(runner.into_os_string());
        wrapped.extend(bwrap_args(policy, argv));
        Ok(WrappedArgv {
            argv: wrapped,
            enforcement: Enforcement::Full,
        })
    }
}

/// The arguments after the runner name: the mount view, then the caller's argv.
fn bwrap_args(policy: &BoundedPolicy, argv: &[OsString]) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--new-session",
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
        "--cap-drop",
        "ALL",
    ]
    .iter()
    .map(OsString::from)
    .collect();

    for root in &policy.writable_roots {
        args.push(OsString::from("--bind"));
        args.push(root.as_os_str().to_os_string());
        args.push(root.as_os_str().to_os_string());
    }

    args.push(OsString::from("--"));
    args.extend(argv.iter().cloned());
    args
}

/// The bubblewrap binary, preferring a system location over `PATH`.
fn find_bwrap() -> Option<PathBuf> {
    let fixed = [
        PathBuf::from("/usr/bin/bwrap"),
        PathBuf::from("/usr/local/bin/bwrap"),
    ];
    if let Some(runner) = fixed.iter().find(|path| path.is_file()) {
        return Some(runner.clone());
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join("bwrap"))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SandboxMode;

    #[test]
    fn bwrap_args_bind_each_root_and_leave_the_network_alone() {
        let policy = BoundedPolicy {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: vec![PathBuf::from("/work")],
        };
        let argv = vec![
            OsString::from("sh"),
            OsString::from("-c"),
            OsString::from("true"),
        ];
        let args = bwrap_args(&policy, &argv);
        let args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(args
            .windows(3)
            .any(|window| window == ["--bind", "/work", "/work"]));
        assert!(
            !args.iter().any(|arg| arg == "--unshare-net"),
            "network access is left as the machine has it"
        );
        let separator = args.iter().position(|arg| arg == "--").expect("separator");
        assert_eq!(&args[separator + 1..], ["sh", "-c", "true"]);
    }
}
