//! What a workspace-write session can reach through the shell.
//!
//! The wrapper is bubblewrap, so these tests run only where it can start. A
//! machine without a working runner skips them rather than failing a build.

use std::path::{Path, PathBuf};
use std::process::Command;

use srud_core::sandbox::SandboxLevel;
use srud_core::tools::{shell::ShellTool, Tool, ToolContext, ToolOutcome};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("srud-workspace-write").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

fn bwrap_works() -> bool {
    Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--", "true"])
        .status()
        .is_ok_and(|status| status.success())
}

fn ctx(dir: &Path, level: SandboxLevel) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
        cancel: tokio_util::sync::CancellationToken::new(),
        sandbox: level.policy(dir),
    }
}

async fn shell(ctx: &ToolContext, command: &str) -> ToolOutcome {
    ShellTool::default()
        .call(ctx, serde_json::json!({ "command": command }))
        .await
        .expect("the arguments match the schema")
}

fn reply(outcome: &ToolOutcome) -> serde_json::Value {
    assert!(
        !outcome.is_error,
        "expected the command to run: {}",
        outcome.output
    );
    serde_json::from_str(&outcome.output).expect("the reply is JSON")
}

/// A path outside the workspace and the temporary roots.
fn outside(name: &str) -> PathBuf {
    PathBuf::from(format!(
        "/var/tmp/srud-workspace-outside-{}-{name}.txt",
        std::process::id()
    ))
}

#[tokio::test]
async fn workspace_write_can_write_inside_its_workspace() {
    if !bwrap_works() {
        return;
    }
    let dir = scratch("inside");
    let outcome = shell(
        &ctx(&dir, SandboxLevel::WorkspaceWrite),
        "echo inside > inside.txt",
    )
    .await;
    let out = reply(&outcome);

    assert_eq!(out["exit_code"], 0);
    assert_eq!(
        std::fs::read_to_string(dir.join("inside.txt")).unwrap(),
        "inside\n"
    );
}

#[tokio::test]
async fn workspace_write_cannot_write_outside_its_roots() {
    if !bwrap_works() {
        return;
    }
    let dir = scratch("outside-denied");
    let target = outside("denied");
    let _ = std::fs::remove_file(&target);

    let outcome = shell(
        &ctx(&dir, SandboxLevel::WorkspaceWrite),
        &format!("echo outside > {}", target.display()),
    )
    .await;
    let out = reply(&outcome);

    assert_ne!(out["exit_code"], 0, "the write should have been refused");
    assert!(
        !target.exists(),
        "the file was not created outside the roots"
    );
}

#[tokio::test]
async fn full_access_can_write_outside_the_workspace() {
    let dir = scratch("outside-allowed");
    let target = outside("allowed");
    if std::fs::write(&target, "probe").is_err() {
        return;
    }
    let _ = std::fs::remove_file(&target);

    let outcome = shell(
        &ctx(&dir, SandboxLevel::FullAccess),
        &format!("echo outside > {}", target.display()),
    )
    .await;
    let out = reply(&outcome);

    assert_eq!(out["exit_code"], 0);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "outside\n");
    let _ = std::fs::remove_file(&target);
}
