//! The `bash` tool: run one shell command in the session's working directory.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::{join, time::timeout};

use super::{failure_text, Tool, ToolContext, ToolError, ToolOutcome};

/// The name the model calls.
const NAME: &str = "bash";

/// The interpreter every command goes through.
///
/// A fixed path rather than `$SHELL`: zsh reads `.zshenv` on every invocation
/// and expands aliases while it parses, so an alias in the user's own startup
/// file would decide what a command means after this tool had finished reading
/// it.
const SHELL: &str = "/bin/sh";

/// Commands that are refused, by the name the shell resolves them to.
///
/// Matched against the command name rather than searched for in the text, so
/// `/bin/rm` and `rm;` are caught while `echo rm` is not. It catches a command
/// that deletes what it names; it does not bound what a command can reach, since
/// redirection (`> file`), `find -delete`, `git clean` and `mv x /dev/null` all
/// destroy files without naming anything here.
const DENIED: &[&str] = &[
    "rm", "rmdir", "dd", "mkfs", "fdisk", "shutdown", "reboot", "halt", "poweroff", "chown",
    "chmod",
];

/// The most one command may print, bounding what this process holds while a pipe
/// is still filling.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// The most one command may return to the model.
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Run one shell command in the working directory and report what it did. `command` is a \
single string handed to /bin/sh, so pipes, redirection and && work. The reply is JSON: \
`stdout`, `stderr`, `exit_code`, and `truncated`. Long output is cut from the front, so \
the end — where a failure shows — is always there. `rm`, `rmdir`, `dd`, `mkfs`, `fdisk`, \
`shutdown`, `reboot`, `halt`, `poweroff`, `chown` and `chmod` are refused; a command that \
needs one is reported as refused, and spelling it differently will be refused too.";

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// The command line, as the shell would read it.
    command: String,
}

/// Runs shell commands from the session's working directory.
#[derive(Debug)]
pub struct BashTool {
    /// How long one command may run before it is stopped.
    timeout: Duration,
}

impl Default for BashTool {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
        }
    }
}

#[async_trait::async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command line to run through /bin/sh."
                }
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let args: Args =
            serde_json::from_value(arguments).map_err(|err| invalid_arguments(err.to_string()))?;

        if let Some(name) = refused_command(&args.command) {
            let message = format!("`{name}` is not available through this tool.");
            let hint = "The refusal is on the program name, so another spelling of it, a full \
                        path, or an alias will be refused too. Do not work around it: say that \
                        the task cannot be done this way.";
            return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
        }

        let mut child = match builder(ctx, &args.command).spawn() {
            Ok(child) => child,
            Err(err) => {
                let message = format!("The command did not start: {err}");
                return Ok(ToolOutcome::failure(failure_text(
                    message,
                    Some(spawn_hint(&err)),
                )));
            }
        };
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        // Drained together while the command runs. Reading one to its end first would
        // leave the child writing into a full pipe, waiting for a reader that is
        // not coming.
        let collected = timeout(self.timeout, async {
            let (out, err) = join!(
                read_capped(stdout, MAX_CAPTURE_BYTES),
                read_capped(stderr, MAX_CAPTURE_BYTES)
            );
            let status = child.wait().await;
            (out, err, status)
        })
        .await;

        let (out, err, status) = match collected {
            Ok(collected) => collected,
            Err(_) => {
                // The signal reaches the shell, not what the shell started: a process
                // tree is not something this holds a handle to.
                let _ = child.start_kill();
                let message = format!("Stopped after {:?} without finishing.", self.timeout);
                let hint = "Whatever the command started may still be running: the signal \
                            reaches the shell, not its children. Check for a leftover process \
                            before retrying, and split the work into steps that finish inside \
                            the limit.";
                return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
            }
        };

        let status = match status {
            Ok(status) => status,
            Err(err) => {
                let message = format!("The command ran but its exit status is unavailable: {err}");
                // Output was read, so whatever the command printed before it ended
                // is still worth reporting.
                let collected_so_far = String::from_utf8_lossy(&out);
                return Ok(ToolOutcome::failure(failure_text(
                    message,
                    Some(&format!("stdout before it ended:\n{collected_so_far}")),
                )));
            }
        };

        let (stdout, truncated) = ending(&String::from_utf8_lossy(&out), MAX_OUTPUT_BYTES);
        let (stderr, _) = ending(&String::from_utf8_lossy(&err), MAX_OUTPUT_BYTES);

        let reply = serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            // The shell ran, so a non-zero exit is the command's own news rather than a
            // failed tool call. `is_error` is for the tool being unable to run it.
            "exit_code": status.code(),
            "truncated": truncated,
        });
        Ok(ToolOutcome::success(reply.to_string()))
    }
}

/// The command set up to run `command` in `ctx`'s working directory.
fn builder(ctx: &ToolContext, command: &str) -> Command {
    let mut child = Command::new(SHELL);
    child
        .arg("-c")
        .arg(command)
        .current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // An abandoned child would keep running with nobody reading what it
        // writes, and eventually block on a full pipe.
        .kill_on_drop(true);

    // Nothing inherited is kept, so a key the agent has no business reading is
    // not one `env` can print. PATH and HOME stay: a shell with neither cannot
    // find a program or read a repository.
    child
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", std::env::var("HOME").unwrap_or_default());
    child
}

/// What to try when the spawn itself failed.
///
/// A spawn that does not happen means the shell is missing or the working
/// directory is gone, and the two call for opposite responses: nothing this tool
/// can fix, or a path the model chose wrong.
fn spawn_hint(err: &std::io::Error) -> &'static str {
    match err.kind() {
        std::io::ErrorKind::NotFound => {
            "Either the working directory does not exist, or the shell is not at /bin/sh. \
             Check the path before running anything else from here."
        }
        std::io::ErrorKind::PermissionDenied => "The shell exists but this process may not run it.",
        _ => {
            "Nothing about the command was run, so retrying it as written will fail the \
              same way."
        }
    }
}

/// Reads a stream to its end, stopping once `cap` bytes have arrived.
async fn read_capped<R>(reader: R, cap: usize) -> Vec<u8>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::new();
    // An error part-way through has already yielded what came before it, and that
    // is worth more to the model than the error.
    let _ = reader.take(cap as u64).read_to_end(&mut buf).await;
    buf
}

/// The last `limit` bytes of `text`, and whether anything was dropped.
fn ending(text: &str, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text.to_owned(), false);
    }
    // A byte index can land inside a character, and half of one is not text.
    let mut at = text.len() - limit;
    while !text.is_char_boundary(at) {
        at += 1;
    }
    (text[at..].to_owned(), true)
}

/// The first command in `script` that [`DENIED`] refuses.
fn refused_command(script: &str) -> Option<String> {
    split(script)
        .iter()
        .filter_map(|segment| segment.split_whitespace().next())
        .filter_map(|token| Path::new(token).file_name().and_then(|name| name.to_str()))
        .find(|name| DENIED.contains(name))
        .map(str::to_owned)
}

/// Splits a command line into the segments a shell would run separately.
///
/// Separators inside quotes are text, so `echo "a && b"` stays one segment and
/// the `&&` in it cannot start a command that was never written.
fn split(script: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for (at, ch) in script.char_indices() {
        if escaped {
            escaped = false;
        } else if let Some(open) = quote {
            if ch == open {
                quote = None;
            }
        } else {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '\\' => escaped = true,
                ';' | '&' | '|' | '\n' | '(' | ')' | '{' | '}' => {
                    segments.push(&script[start..at]);
                    start = at + ch.len_utf8();
                }
                _ => {}
            }
        }
    }
    segments.push(&script[start..]);
    segments
}

/// The error for arguments the schema permits but the tool cannot act on.
fn invalid_arguments(message: impl Into<String>) -> ToolError {
    ToolError::InvalidArguments {
        name: NAME.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::super::FAILURE_MARKER;
    use super::*;

    /// A directory for one test to run in, emptied first so a rerun starts from
    /// the same place.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("srud-bash-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
        }
    }

    /// Runs `command`, expecting well-formed arguments.
    async fn run(dir: &std::path::Path, command: &str) -> ToolOutcome {
        BashTool::default()
            .call(&ctx(dir), serde_json::json!({ "command": command }))
            .await
            .expect("the arguments match the schema")
    }

    /// The reply, asserting the tool ran the command.
    fn reply(outcome: &ToolOutcome) -> serde_json::Value {
        assert!(!outcome.is_error, "expected a run: {}", outcome.output);
        serde_json::from_str(&outcome.output).expect("the reply is JSON")
    }

    /// The refusal message, asserting the tool refused before running anything.
    fn refusal(outcome: &ToolOutcome) -> String {
        assert!(outcome.is_error, "expected a refusal: {}", outcome.output);
        outcome.output.clone()
    }

    #[tokio::test]
    async fn reports_what_a_command_printed() {
        let dir = scratch("prints");
        let out = reply(&run(&dir, "echo hello").await);

        assert_eq!(out["stdout"], "hello\n");
        assert_eq!(out["stderr"], "");
        assert_eq!(out["exit_code"], 0);
        assert_eq!(out["truncated"], false);
    }

    #[tokio::test]
    async fn a_failing_command_is_reported_rather_than_a_failed_tool() {
        // The exit code is the command's own news, so the reply is a success:
        // what failed was the command, not the tool that ran it.
        let dir = scratch("failing");
        let outcome = run(&dir, "echo out; echo err >&2; exit 3").await;
        let out = reply(&outcome);

        assert_eq!(out["stdout"], "out\n");
        assert_eq!(out["stderr"], "err\n");
        assert_eq!(out["exit_code"], 3);
        assert!(!outcome.is_error);
    }

    #[tokio::test]
    async fn keeps_the_end_of_output_longer_than_the_reply() {
        let dir = scratch("long");
        // The marker comes last, so it is in the reply only if the front was the part
        // given up.
        let command =
            "i=0; while [ $i -lt 20000 ]; do echo padding; i=$((i+1)); done; echo THE-END";
        let out = reply(&run(&dir, command).await);

        assert_eq!(out["truncated"], true);
        assert!(
            out["stdout"]
                .as_str()
                .expect("stdout")
                .ends_with("THE-END\n"),
            "the end of the output is kept"
        );
        assert!(
            out["stdout"].as_str().expect("stdout").len() <= MAX_OUTPUT_BYTES,
            "the reply stays within its budget"
        );
    }

    #[tokio::test]
    async fn runs_in_the_working_directory() {
        let dir = scratch("cwd");
        std::fs::write(dir.join("marker.txt"), "here").expect("fixture");
        let out = reply(&run(&dir, "ls").await);

        assert!(
            out["stdout"]
                .as_str()
                .expect("stdout")
                .contains("marker.txt"),
            "the command saw the working directory: {}",
            out["stdout"]
        );
    }

    #[tokio::test]
    async fn refuses_a_denied_command() {
        for command in [
            "rm note.txt",
            "/bin/rm -rf .",
            "ls; rm -rf /",
            "ls && rm note.txt",
            "ls\nrm note.txt",
            "echo hi | rm note.txt",
            "chmod 777 note.txt",
        ] {
            let dir = scratch("refused");
            std::fs::write(dir.join("note.txt"), "here").expect("fixture");
            let message = refusal(&run(&dir, command).await);

            assert!(
                message.contains("rm") || message.contains("chmod"),
                "the refusal names the command: {message} ({command})"
            );
            assert!(
                dir.join("note.txt").exists(),
                "the file is still there after {command}"
            );
        }
    }

    #[tokio::test]
    async fn leaves_text_that_only_looks_like_a_denied_command() {
        // Only the name the shell would run is refused. Text that merely contains
        // one is not a command.
        for command in [
            "echo rm",
            "echo 'rm -rf /'",
            "grep -r rm .",
            "echo done > rm",
        ] {
            let dir = scratch("allowed");
            let outcome = run(&dir, command).await;
            assert!(
                !outcome.is_error,
                "{command} should have run: {}",
                outcome.output
            );
        }
    }

    #[tokio::test]
    async fn does_not_hand_the_command_the_hosts_environment() {
        let dir = scratch("env");
        // Set here, so the assertion does not rest on the developer's own shell.
        std::env::set_var("SRUD_BASH_TEST_SECRET", "leaked");
        let out = reply(&run(&dir, "echo \"[$SRUD_BASH_TEST_SECRET]\"").await);
        std::env::remove_var("SRUD_BASH_TEST_SECRET");

        assert_eq!(
            out["stdout"], "[]\n",
            "the variable did not reach the command"
        );
    }

    #[tokio::test]
    async fn stops_a_command_that_outlasts_its_budget() {
        let dir = scratch("timeout");
        let tool = BashTool {
            timeout: Duration::from_millis(150),
        };
        let outcome = tool
            .call(&ctx(&dir), serde_json::json!({ "command": "sleep 30" }))
            .await
            .expect("the arguments match the schema");

        assert!(outcome.is_error);
        assert!(
            outcome.output.contains("Stopped after"),
            "the reply says it was stopped: {}",
            outcome.output
        );
        assert!(
            outcome.output.contains("may still be running"),
            "the reply says what is left over: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn a_command_that_cannot_be_started_is_a_failed_outcome() {
        // A path nothing has created, so the spawn itself fails. `scratch` would have
        // made the directory, and then there would be nothing to fail.
        let dir = std::env::temp_dir()
            .join("srud-bash-tests")
            .join("never-created");
        let _ = std::fs::remove_dir_all(&dir);
        let outcome = run(&dir, "echo hello").await;

        assert!(
            outcome.is_error,
            "a missing working directory is the tool's problem"
        );
        assert!(
            outcome.output.contains("did not start"),
            "the reply says the command never started: {}",
            outcome.output
        );
        assert!(
            outcome.output.contains("working directory does not exist"),
            "the reply names the likely cause: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn a_command_killed_by_a_signal_reports_no_exit_code() {
        // There is no exit code, so the reply says null rather than a number standing
        // in for one.
        let dir = scratch("signalled");
        let out = reply(&run(&dir, "kill -TERM $$").await);

        assert_eq!(out["exit_code"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn output_larger_than_the_capture_budget_does_not_hang() {
        // A pipe holds far less than the capture budget, so a reader that stopped at the
        // budget would leave the command blocked forever. The command has to
        // finish, and the reply says the output was cut.
        let dir = scratch("flood");
        let command = "i=0; while [ $i -lt 60000 ]; do echo a-fairly-long-line-of-padding; \
                      i=$((i+1)); done";
        let outcome = run(&dir, command).await;
        let out = reply(&outcome);

        assert_eq!(out["truncated"], true);
        assert!(!outcome.is_error, "the command finished on its own");
    }

    #[tokio::test]
    async fn every_refusal_is_marked_so_it_can_be_told_from_a_result() {
        // A command that exits 1 is a result, not a refusal, and the two have to be
        // separable from the text alone. That is what the marker is for.
        let dir = scratch("marked");
        let refused = refusal(&run(&dir, "rm note.txt").await);
        let failed_but_ran = reply(&run(&dir, "exit 1").await);

        assert!(
            refused.starts_with(FAILURE_MARKER),
            "a refusal is marked: {refused}"
        );
        assert!(
            failed_but_ran["exit_code"] == 1 && failed_but_ran["truncated"] == false,
            "a command that ran is a result carrying its exit code, not a refusal"
        );
    }

    #[tokio::test]
    async fn a_result_never_carries_the_marker() {
        // The marker is the only thing telling the two apart, so a result that
        // happened to print it would make itself unreadable.
        let dir = scratch("collision");
        let outcome = run(&dir, "echo '[tool error] not really'").await;
        reply(&outcome);

        assert!(!outcome.is_error, "the command ran, so it is a result");
        assert!(
            !outcome.output.starts_with(FAILURE_MARKER),
            "the marker is only at the front: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn a_refusal_says_working_around_it_will_not_help() {
        let dir = scratch("no-workaround");
        let message = refusal(&run(&dir, "rm note.txt").await);

        assert!(
            message.contains("another spelling"),
            "the refusal says the name is what is refused: {message}"
        );
        assert!(
            message.contains("Do not work around it"),
            "the refusal tells the model not to try: {message}"
        );
    }

    #[tokio::test]
    async fn an_unknown_argument_is_refused() {
        let dir = scratch("unknown-argument");
        let err = BashTool::default()
            .call(
                &ctx(&dir),
                serde_json::json!({ "command": "echo hi", "cwd": "/etc" }),
            )
            .await
            .expect_err("unknown fields are refused rather than ignored");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn refuses_the_commands_it_lists() {
        // A command the description names and the tool does not refuse is one the model
        // tries, and then works around.
        for name in DENIED {
            let script = refused_command(&format!("{name} --help"));
            assert_eq!(
                script.as_deref(),
                Some(*name),
                "{name} is listed but not refused"
            );
            assert!(
                DESCRIPTION.contains(name),
                "{name} is refused but not named in the description"
            );
        }
    }

    #[test]
    fn finds_a_denied_command_wherever_it_appears_in_the_line() {
        assert_eq!(refused_command("rm x").as_deref(), Some("rm"));
        assert_eq!(refused_command("/usr/bin/rm x").as_deref(), Some("rm"));
        assert_eq!(refused_command("a | b ; rm").as_deref(), Some("rm"));
        assert_eq!(refused_command("a && rm && b").as_deref(), Some("rm"));
        assert_eq!(refused_command("(rm)").as_deref(), Some("rm"));
        assert_eq!(refused_command("echo rm").as_deref(), None);
        assert_eq!(refused_command("echo 'a; rm'").as_deref(), None);
        assert_eq!(refused_command("").as_deref(), None);
    }
}
