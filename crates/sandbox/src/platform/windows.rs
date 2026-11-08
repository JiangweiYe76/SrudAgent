//! The Windows runner: a launcher process around the caller's argv.
//!
//! The launcher builds a write-restricted token, grants write access under
//! each writable root to the SID derived for it, and spawns the command with
//! `CreateProcessAsUserW`. The wrap stays a pure argv transform; all the Win32
//! work happens in the child, so the runtime never holds a restricted token
//! itself.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::{
    BoundedPolicy, Enforcement, SandboxBackend, SandboxMode, SandboxUnavailable, WrappedArgv,
};

/// The launcher binary's name, without the platform suffix.
const LAUNCHER_NAME: &str = "srud-sandbox-launcher";

/// The backend that hands the command to the restricted-token launcher.
pub(super) struct WindowsBoundary;

impl SandboxBackend for WindowsBoundary {
    fn wrap_argv(
        &self,
        argv: &[OsString],
        policy: &BoundedPolicy,
    ) -> Result<WrappedArgv, SandboxUnavailable> {
        let launcher = find_launcher().ok_or(SandboxUnavailable::RunnerMissing {
            program: LAUNCHER_NAME,
        })?;
        let mut wrapped = Vec::with_capacity(argv.len() + policy.writable_roots.len() * 2 + 4);
        wrapped.push(launcher.into_os_string());
        wrapped.extend(launcher_args(policy, argv));
        Ok(WrappedArgv {
            argv: wrapped,
            enforcement: Enforcement::Partial {
                missing: vec![
                    "reads are unrestricted".to_owned(),
                    "hard-link aliases bypass path ACLs".to_owned(),
                ],
            },
        })
    }
}

/// The policy arguments between the launcher and the `--` separator.
fn launcher_args(policy: &BoundedPolicy, argv: &[OsString]) -> Vec<OsString> {
    let mode = match policy.mode {
        SandboxMode::ReadOnly => "read-only",
        SandboxMode::WorkspaceWrite => "workspace-write",
    };
    let mut args = vec![OsString::from("--mode"), OsString::from(mode)];
    if policy.mode == SandboxMode::WorkspaceWrite {
        for root in &policy.writable_roots {
            args.push(OsString::from("--root"));
            args.push(root.as_os_str().to_os_string());
        }
    }
    args.push(OsString::from("--"));
    args.extend(argv.iter().cloned());
    args
}

/// Where the launcher is: an explicit override, beside this executable (one
/// directory up for a test binary in `deps`), or on `PATH`.
fn find_launcher() -> Option<PathBuf> {
    let name = format!("{LAUNCHER_NAME}{}", std::env::consts::EXE_SUFFIX);
    if let Some(override_path) = std::env::var_os("SRUD_SANDBOX_LAUNCHER") {
        let path = PathBuf::from(override_path);
        return path.is_file().then_some(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // Beside this executable, or one directory up when it is a test
            // binary running from `deps`.
            let beside = dir.join(&name);
            if beside.is_file() {
                return Some(beside);
            }
            if let Some(up) = dir.parent() {
                let beside = up.join(&name);
                if beside.is_file() {
                    return Some(beside);
                }
            }
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|dir| dir.join(&name))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn workspace_write_names_every_root_in_order() {
        let policy = BoundedPolicy {
            mode: SandboxMode::WorkspaceWrite,
            writable_roots: vec![PathBuf::from(r"C:\work"), PathBuf::from(r"C:\temp")],
        };
        let argv = vec![
            OsString::from("cmd.exe"),
            OsString::from("/C"),
            OsString::from("echo hi"),
        ];
        assert_eq!(
            strings(&launcher_args(&policy, &argv)),
            [
                "--mode",
                "workspace-write",
                "--root",
                r"C:\work",
                "--root",
                r"C:\temp",
                "--",
                "cmd.exe",
                "/C",
                "echo hi",
            ]
        );
    }

    #[test]
    fn read_only_carries_no_roots() {
        let policy = BoundedPolicy {
            mode: SandboxMode::ReadOnly,
            writable_roots: Vec::new(),
        };
        let argv = vec![OsString::from("cmd.exe")];
        assert_eq!(
            strings(&launcher_args(&policy, &argv)),
            ["--mode", "read-only", "--", "cmd.exe"]
        );
    }

    #[test]
    fn the_separator_shields_the_inner_argv() {
        let policy = BoundedPolicy {
            mode: SandboxMode::ReadOnly,
            writable_roots: Vec::new(),
        };
        let argv = vec![
            OsString::from("tool"),
            OsString::from("--mode"),
            OsString::from("full-access"),
        ];
        let args = strings(&launcher_args(&policy, &argv));
        let separator = args.iter().position(|arg| arg == "--").expect("separator");
        assert_eq!(&args[separator + 1..], ["tool", "--mode", "full-access"]);
    }
}
