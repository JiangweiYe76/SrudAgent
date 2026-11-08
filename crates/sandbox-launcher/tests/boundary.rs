#![cfg(windows)]

//! The boundary, exercised through the real launcher binary: what it may
//! write, what it may not, and what it passes back.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_srud-sandbox-launcher");

/// The launcher's exit code when it cannot prepare the boundary.
const SETUP_FAILURE: i32 = 125;

/// A directory for one test, emptied first so a rerun starts from the same
/// place.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("srud-launcher-tests").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

fn comspec() -> String {
    std::env::var("COMSPEC").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into())
}

/// A cmd command line; `/S` makes cmd read the quoted command as written.
fn cmd(command: &str) -> Vec<String> {
    vec![comspec(), "/S".into(), "/C".into(), command.into()]
}

/// A PowerShell command line; the interpreter Windows always ships.
fn powershell(script: &str) -> Vec<String> {
    vec![
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
        "-NoProfile".into(),
        "-Command".into(),
        script.into(),
    ]
}

/// Runs the launcher with the given policy and inner command.
fn launch(mode: &str, roots: &[&Path], cwd: &Path, inner: &[String]) -> Output {
    let mut command = Command::new(LAUNCHER);
    command.arg("--mode").arg(mode);
    for root in roots {
        command.arg("--root").arg(root);
    }
    command.arg("--").args(inner).current_dir(cwd);
    if let Some(root) = roots.first() {
        // The real policy grants the session temp dir as a writable root too;
        // point the child's temp writes at the workspace so they stay inside.
        command.env("TMP", root).env("TEMP", root);
    }
    command.output().expect("the launcher starts")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn writes_inside_the_workspace_including_new_subdirectories() {
    let dir = scratch("write-inside");
    let out = launch(
        "workspace-write",
        &[&dir],
        &dir,
        &cmd("echo inside>made.txt & mkdir sub & echo nested>sub\\made.txt"),
    );
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(
        std::fs::read_to_string(dir.join("made.txt"))
            .expect("file")
            .trim(),
        "inside"
    );
    assert!(
        dir.join("sub").join("made.txt").is_file(),
        "the grant inherits into directories the command creates"
    );
}

#[test]
fn writes_outside_the_workspace_are_refused() {
    let dir = scratch("write-outside");
    let outside = scratch("write-outside-target");
    let target = outside.join("made.txt");
    // PowerShell rather than cmd: a script carrying a quoted path would need
    // cmd's own escaping rules, which the standard command-line encoding is
    // not.
    let script = format!(
        "try {{ Set-Content -Path '{}' -Value out -ErrorAction Stop; exit 0 }} catch {{ exit 5 }}",
        target.display()
    );
    let out = launch("workspace-write", &[&dir], &dir, &powershell(&script));

    assert_eq!(out.status.code(), Some(5), "the write was refused");
    assert!(!target.exists(), "nothing was created outside the root");
}

#[test]
fn read_only_refuses_writes_inside_the_workspace() {
    let dir = scratch("read-only");
    let out = launch("read-only", &[], &dir, &cmd("echo x> made.txt"));

    assert_ne!(out.status.code(), Some(0), "the write was refused");
    assert!(!dir.join("made.txt").exists());
}

#[test]
fn reads_outside_the_workspace_still_work() {
    let dir = scratch("read-outside");
    let outside = scratch("read-outside-source");
    let source = outside.join("secret.txt");
    std::fs::write(&source, "readable").expect("fixture");

    let script = format!("Get-Content -Raw -LiteralPath '{}'", source.display());
    let out = launch("workspace-write", &[&dir], &dir, &powershell(&script));

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(
        text(&out.stdout).contains("readable"),
        "reads are not restricted: {}",
        text(&out.stdout)
    );
}

#[test]
fn the_exit_code_passes_through() {
    let dir = scratch("exit-code");
    let out = launch("workspace-write", &[&dir], &dir, &cmd("exit 7"));
    assert_eq!(out.status.code(), Some(7));
}

#[test]
fn powershell_starts_and_passes_both_streams_through() {
    let dir = scratch("powershell");
    let script =
        "Write-Output ok; [Console]::Error.WriteLine('err'); Set-Content -Path ps-made.txt \
                  -Value ps";
    let out = launch("workspace-write", &[&dir], &dir, &powershell(script));

    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("ok"), "{}", text(&out.stdout));
    assert!(text(&out.stderr).contains("err"), "{}", text(&out.stderr));
    assert!(dir.join("ps-made.txt").is_file(), "PowerShell wrote inside");
}

#[test]
fn a_setup_failure_never_runs_the_command() {
    let dir = scratch("setup-failure");
    // workspace-write without a --root cannot be prepared.
    let out = launch("workspace-write", &[], &dir, &cmd("echo ran> marker.txt"));

    assert_eq!(out.status.code(), Some(SETUP_FAILURE));
    assert!(text(&out.stderr).contains("srud-sandbox-launcher:"));
    assert!(!dir.join("marker.txt").exists(), "the command never ran");
    assert!(out.stdout.is_empty());
}

#[test]
fn an_unknown_mode_is_refused() {
    let dir = scratch("unknown-mode");
    let out = launch("bogus", &[], &dir, &cmd("echo ran"));
    assert_eq!(out.status.code(), Some(SETUP_FAILURE));
    assert!(text(&out.stderr).contains("unknown mode"));
}

#[test]
fn a_root_that_is_not_there_is_refused() {
    let dir = scratch("missing-root");
    let missing = dir.join("never-created");
    let out = launch("workspace-write", &[&missing], &dir, &cmd("echo ran"));
    assert_eq!(out.status.code(), Some(SETUP_FAILURE));
    assert!(text(&out.stderr).contains("not usable"));
}
