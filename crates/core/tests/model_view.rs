//! What a model actually reads after a tool call.
//!
//! `ToolOutcome` carries `is_error`, and the desktop UI reads it. The provider
//! carries tool output as a string, so the text is the only thing the model gets.
//! These tests pin that text, since nothing else about a failure reaches the
//! model.

use srud_core::tools::{
    bash::BashTool, failure_text, read::ReadTool, Tool, ToolContext, ToolOutcome, FAILURE_MARKER,
};

use std::path::{Path, PathBuf};

/// A directory holding one file, emptied first so a rerun starts from the same
/// place.
fn scratch(name: &str, file: &str, contents: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("srud-model-view").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    std::fs::write(dir.join(file), contents).expect("fixture file");
    dir
}

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
    }
}

async fn read(dir: &Path, path: &str) -> ToolOutcome {
    ReadTool
        .call(&ctx(dir), serde_json::json!({ "path": path }))
        .await
        .expect("the arguments match the schema")
}

async fn bash(dir: &Path, command: &str) -> ToolOutcome {
    BashTool::default()
        .call(&ctx(dir), serde_json::json!({ "command": command }))
        .await
        .expect("the arguments match the schema")
}

/// The four things a model can be looking at after a turn: a result from either
/// tool, or a failure from either.
#[tokio::test]
async fn the_model_sees_four_distinguishable_shapes() {
    let dir = scratch("shapes", "note.txt", "one\ntwo\n");

    let read_ok = read(&dir, "note.txt").await;
    let read_missing = read(&dir, "nope.txt").await;
    let bash_ok = bash(&dir, "echo hello").await;
    let bash_refused = bash(&dir, "rm note.txt").await;

    println!("── read, found ─────────────────────────");
    println!("{}", read_ok.output);
    println!("── read, missing ──────────────────────");
    println!("{}", read_missing.output);
    println!("── bash, ran ──────────────────────────");
    println!("{}", bash_ok.output);
    println!("── bash, refused ──────────────────────");
    println!("{}", bash_refused.output);

    assert!(!read_ok.is_error && !bash_ok.is_error);
    assert!(read_missing.is_error && bash_refused.is_error);

    // A failure is recognisable without parsing prose, whichever tool produced it.
    assert!(read_missing.output.starts_with(FAILURE_MARKER));
    assert!(bash_refused.output.starts_with(FAILURE_MARKER));
}

/// A result and a failure are distinguishable by the first line alone, which is
/// what lets a model branch without reading the rest.
#[tokio::test]
async fn the_first_line_is_enough_to_tell_them_apart() {
    let dir = scratch("first-line", "note.txt", "one\n");

    for outcome in [
        read(&dir, "note.txt").await,
        bash(&dir, "echo hi").await,
        read(&dir, "nope.txt").await,
        bash(&dir, "dd if=/dev/zero of=x").await,
        read(&dir, ".").await,
    ] {
        let first = outcome.output.lines().next().unwrap_or_default();
        let is_failure = outcome.output.starts_with(FAILURE_MARKER);
        assert_eq!(
            is_failure, outcome.is_error,
            "the marker and is_error disagree for: {first}"
        );
        if !is_failure {
            assert!(
                serde_json::from_str::<serde_json::Value>(&outcome.output).is_ok(),
                "a result parses as JSON, and this one did not: {first}"
            );
        }
    }
}

/// Every way a tool call can fail arrives marked, including the ones the loop
/// produces rather than the tool.
///
/// Arguments the schema permits but a tool cannot act on come back as a
/// `ToolError`, which the turn loop turns into a failure of its own. That path is
/// the one a model reaches by misreading its own output — told a file has no line
/// 9999, it asks for offset 0 — so it has to be indistinguishable from any other
/// failure.
#[tokio::test]
async fn a_rejected_argument_reaches_the_model_marked() {
    let dir = scratch("rejected", "note.txt", "one\ntwo\n");
    let outcome = read(&dir, "note.txt").await;
    assert!(
        !outcome.is_error,
        "the same call with valid arguments succeeds"
    );

    // What the loop does with the tool's own `ToolError`, reproduced here rather
    // than run through a turn: `turn.rs` asserts the shape end to end.
    let rejected = failure_text(
        "The arguments were rejected before anything ran: \
         invalid arguments for tool read: offset must be a 1-indexed line number",
        Some("Nothing was run. Fix the arguments and call the tool again."),
    );
    assert!(rejected.starts_with(FAILURE_MARKER));
    assert!(
        rejected.contains("before anything ran"),
        "it says the tool did not run, so the model does not think a file was read: {rejected}"
    );
}

/// What a model gains from the extra sentence: the next step, not just the cause.
#[tokio::test]
async fn a_failure_carries_what_to_try() {
    let dir = scratch("hints", "note.txt", "one\n");
    let missing = read(&dir, "nope.txt").await;
    let directory = read(&dir, ".").await;
    let refused = bash(&dir, "rm note.txt").await;

    // The cause alone would leave these open; the second part closes them.
    assert!(
        missing
            .output
            .contains("resolved against the working directory"),
        "a missing file says how the path was read: {}",
        missing.output
    );
    assert!(
        directory.output.contains("directory"),
        "a directory says what it is: {}",
        directory.output
    );
    assert!(
        refused.output.contains("Do not work around it"),
        "a refusal says not to retry it differently: {}",
        refused.output
    );

    // Each failure has a message and then advice, so advice is never the whole
    // reply.
    for outcome in [missing, directory, refused] {
        let (_, advice) = outcome
            .output
            .split_once("\n\n")
            .expect("message then advice");
        assert!(!advice.trim().is_empty(), "the advice has content");
    }
}
