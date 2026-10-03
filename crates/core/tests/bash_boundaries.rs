//! What the `bash` tool's own boundary does and does not catch.
//!
//! Each refused case has a file behind it that survives. Each allowed case is one
//! where refusing would be a false positive, and a tool that refuses those stops
//! being usable.

use std::path::{Path, PathBuf};

use srud_core::{
    tools::{bash::BashTool, ToolContext},
    Tool,
};

/// A directory to run in, emptied first so a rerun starts from the same place.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("srud-bash-boundaries").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    std::fs::write(dir.join("note.txt"), "still here").expect("fixture file");
    dir
}

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
    }
}

/// Runs `command` and reports whether the tool refused it.
async fn refused(dir: &Path, command: &str) -> bool {
    let outcome = BashTool::default()
        .call(&ctx(dir), serde_json::json!({ "command": command }))
        .await
        .expect("the arguments match the schema");
    outcome.is_error
}

/// The ways a deleted program can be named, none of which a caller should get
/// past the boundary. The file behind each one is checked afterwards.
#[tokio::test]
async fn a_deleted_program_is_refused_however_it_is_named() {
    for (name, command) in [
        ("plain", "rm note.txt"),
        ("absolute path", "/bin/rm note.txt"),
        ("relative path", "./rm note.txt"),
        ("after a semicolon", "true; rm note.txt"),
        ("after &&", "true && rm note.txt"),
        ("after a pipe", "cat note.txt | rm note.txt"),
        ("on its own line", "true\nrm note.txt"),
        ("inside a subshell", "(rm note.txt)"),
        ("with a full path in /usr/bin", "/usr/bin/rm note.txt"),
        ("with a trailing path segment", "/bin/../bin/rm note.txt"),
        ("with options first", "rm -f note.txt"),
    ] {
        let dir = scratch(name);
        assert!(
            refused(&dir, command).await,
            "`{command}` should have been refused"
        );
        assert!(
            dir.join("note.txt").exists(),
            "the file survived `{command}`"
        );
    }
}

#[tokio::test]
async fn every_refused_program_leaves_its_file_alone() {
    for name in [
        "rm", "rmdir", "dd", "mkfs", "fdisk", "shutdown", "reboot", "halt", "poweroff", "chown",
        "chmod",
    ] {
        let dir = scratch(&format!("program-{name}"));
        assert!(
            refused(&dir, &format!("{name} note.txt")).await,
            "{name} should have been refused"
        );
        assert!(
            dir.join("note.txt").exists(),
            "{name} did not reach the file"
        );
    }
}

/// A boundary that reads too much turns ordinary work into an error, and the
/// model starts reaching for spellings that do get through. These are the cases
/// where refusing would be wrong.
#[tokio::test]
async fn a_refused_program_in_text_is_not_a_refusal() {
    for (name, command) in [
        ("echoed", "echo rm"),
        ("in single quotes", "echo 'rm -rf /'"),
        ("in double quotes", "echo \"chmod 777\""),
        ("as a grep argument", "grep -r rm ."),
        ("as a filename", "echo done > rm"),
        ("quoted separator inside", "echo 'a && rm'"),
        ("escaped separator inside", "echo a \\&\\& rm"),
    ] {
        let dir = scratch(name);
        assert!(
            !refused(&dir, command).await,
            "`{command}` should have run: the refused name is text, not a command"
        );
    }
}

/// Each of these destroys a file while naming nothing on it, which is what a list
/// of program names cannot catch. A refusal that stopped them would be stopping
/// ordinary work.
///
/// They are pinned so the claim stays true of the tool as it is.
#[tokio::test]
async fn what_the_boundary_does_not_cover() {
    let cases = [
        ("redirection truncates a file", "echo replaced > note.txt"),
        ("truncate empties one", "truncate -s 0 note.txt"),
        (
            "git checkout restores over it",
            "git checkout -- note.txt 2>/dev/null || true",
        ),
    ];

    for (name, command) in cases {
        let dir = scratch(&name.replace(' ', "-"));
        assert!(
            !refused(&dir, command).await,
            "`{command}` names no refused program, so it is not caught"
        );
    }
}

/// What the environment the command starts in looks like from inside.
#[tokio::test]
async fn the_command_sees_a_bare_environment() {
    let dir = scratch("environment");
    std::env::set_var("SRUD_BOUNDARY_PROBE", "leaked");
    let outcome = BashTool::default()
        .call(
            &ctx(&dir),
            serde_json::json!({ "command": "echo \"[${SRUD_BOUNDARY_PROBE}]\"; echo \"[$(env | grep -c SRUD_BOUNDARY_PROBE)]\"" }),
        )
        .await
        .expect("the arguments match the schema");
    std::env::remove_var("SRUD_BOUNDARY_PROBE");

    let reply: serde_json::Value =
        serde_json::from_str(&outcome.output).expect("the reply is JSON");
    let stdout = reply["stdout"].as_str().expect("stdout");
    assert_eq!(
        stdout, "[]\n[0]\n",
        "the variable did not reach the command"
    );
}
