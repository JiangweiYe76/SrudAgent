//! The Windows sandbox launcher.
//!
//! The sandbox crate hands the shell a wrapped argv of the shape
//! `[launcher, --mode <mode>, (--root <path>)…, --, …inner argv…]`. This binary
//! parses that contract, builds a restricted token, grants write access under
//! each writable root to the SID derived for it, and spawns the inner command
//! with `CreateProcessAsUserW`. It returns the child's exit status unchanged.
//! On any setup failure it exits non-zero without running the command, so the
//! boundary is fail-closed.

use std::ffi::OsString;
use std::path::PathBuf;

#[cfg(windows)]
mod win;

/// The exit code the launcher uses when it cannot prepare the boundary. The
/// command never runs in that case; the caller sees this status and the
/// launcher's message on stderr.
const SETUP_FAILURE: i32 = 125;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let invocation = match Invocation::parse(&args) {
        Ok(invocation) => invocation,
        Err(message) => fail(&message),
    };

    #[cfg(windows)]
    {
        match win::run(&invocation) {
            Ok(code) => std::process::exit(code as i32),
            Err(err) => fail(&err.0),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = invocation;
        fail("the sandbox launcher only runs on Windows");
    }
}

/// The parsed launcher contract.
#[cfg_attr(not(windows), allow(dead_code))]
struct Invocation {
    /// The bounded mode to enforce.
    mode: Mode,
    /// The only directories a workspace-write child may modify.
    roots: Vec<PathBuf>,
    /// The exact command to run inside the boundary.
    inner: Vec<OsString>,
}

/// The write shape the launcher enforces.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    ReadOnly,
    WorkspaceWrite,
}

impl Invocation {
    fn parse(args: &[OsString]) -> Result<Self, String> {
        let mut mode = None;
        let mut roots = Vec::new();
        let mut inner = None;

        let mut index = 0;
        while index < args.len() {
            match args[index].to_str() {
                Some("--mode") => {
                    index += 1;
                    let value = args
                        .get(index)
                        .and_then(|arg| arg.to_str())
                        .ok_or("--mode needs a value")?;
                    mode = Some(match value {
                        "read-only" => Mode::ReadOnly,
                        "workspace-write" => Mode::WorkspaceWrite,
                        other => return Err(format!("unknown mode `{other}`")),
                    });
                }
                Some("--root") => {
                    index += 1;
                    let value = args.get(index).ok_or("--root needs a value")?;
                    roots.push(PathBuf::from(value));
                }
                Some("--") => {
                    inner = Some(args[index + 1..].to_vec());
                    break;
                }
                other => return Err(format!("unexpected argument `{other:?}`")),
            }
            index += 1;
        }

        let mode = mode.ok_or("missing --mode")?;
        let inner = inner.ok_or("missing `--` separator and command")?;
        if inner.is_empty() {
            return Err("no command after `--`".into());
        }
        if mode == Mode::WorkspaceWrite && roots.is_empty() {
            return Err("workspace-write needs at least one --root".into());
        }
        if mode == Mode::ReadOnly && !roots.is_empty() {
            return Err("read-only takes no --root".into());
        }
        Ok(Self { mode, roots, inner })
    }
}

/// Reports a setup failure and exits without running the command.
fn fail(message: &str) -> ! {
    eprintln!("srud-sandbox-launcher: {message}");
    std::process::exit(SETUP_FAILURE);
}
