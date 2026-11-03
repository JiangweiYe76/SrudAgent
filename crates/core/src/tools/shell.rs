//! The `shell` tool: run one shell command in the session's working directory.

use std::ffi::OsString;
#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::join;
use tokio::process::Command;

use srud_sandbox::{wrap_argv, SandboxPolicy, SandboxUnavailable};

use super::{failure_text, Tool, ToolContext, ToolError, ToolOutcome};

/// The name the model calls.
const NAME: &str = "shell";

/// The syntax a shell reads.
///
/// It decides two things that follow from each other: how a command line is cut
/// into the commands it would run, and whether a program name is compared as
/// written or case-folded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    /// `/bin/sh` and its relatives.
    ///
    /// Windows never resolves to it, but the enum still has to name it: the
    /// denials and the reading of a command line are the same code on both
    /// platforms.
    #[cfg_attr(windows, allow(dead_code))]
    Posix,
    /// PowerShell, and cmd as the fallback beneath it.
    #[cfg_attr(unix, allow(dead_code))]
    Windows,
}

/// The interpreter commands are handed to.
///
/// Resolved once, when the tool is built: the program, the arguments that make
/// it read a command string, and the syntax it reads are one decision, and the
/// description the model reads follows from it.
#[derive(Debug, Clone)]
struct Shell {
    /// The program to start.
    program: PathBuf,
    /// The arguments that come before the command string.
    args: &'static [&'static str],
    /// The syntax the program reads.
    dialect: Dialect,
    /// How the program is named to the model.
    label: &'static str,
}

impl Shell {
    /// The interpreter to run commands in.
    ///
    /// A fixed shell rather than `$SHELL`: zsh reads `.zshenv` on every
    /// invocation and expands aliases while it parses, so an alias in the user's
    /// own startup file would decide what a command means after this tool had
    /// finished reading it. That is also why every PowerShell layer is started
    /// with `-NoProfile`.
    #[cfg(unix)]
    fn detect() -> Self {
        Self {
            program: PathBuf::from("/bin/sh"),
            args: &["-c"],
            dialect: Dialect::Posix,
            label: "/bin/sh",
        }
    }

    /// The interpreter to run commands in.
    ///
    /// Windows resolves in three layers — PowerShell 7, then the PowerShell
    /// Windows ships, then cmd — and never looks for a POSIX shell. The two
    /// `bash.exe` names a Windows machine can have are both wrong answers: a Git
    /// Bash is a shell the user did not choose, and the one in `WindowsApps` is
    /// the WSL launcher, which would run the command inside a Linux virtual
    /// machine with a different view of the same files.
    #[cfg(windows)]
    fn detect() -> Self {
        for (binary, fallbacks, label) in [
            ("pwsh", PWSH_FALLBACK_PATHS, "pwsh"),
            ("powershell", POWERSHELL_FALLBACK_PATHS, "powershell"),
        ] {
            if let Some(program) = find_powershell(binary, fallbacks) {
                return Self {
                    program,
                    args: &["-NoProfile", "-Command"],
                    dialect: Dialect::Windows,
                    label,
                };
            }
        }

        Self {
            program: comspec(),
            args: &["/C"],
            dialect: Dialect::Windows,
            label: "cmd.exe",
        }
    }
}

/// Where PowerShell 7 is normally installed.
#[cfg(windows)]
const PWSH_FALLBACK_PATHS: &[&str] = &[r"C:\Program Files\PowerShell\7\pwsh.exe"];

/// Where the PowerShell Windows ships is always installed.
#[cfg(windows)]
const POWERSHELL_FALLBACK_PATHS: &[&str] =
    &[r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"];

/// The first `binary` on `PATH` that could be started, or one of `fallbacks`.
#[cfg(windows)]
fn find_powershell(binary: &str, fallbacks: &[&str]) -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .map(|directory| directory.join(format!("{binary}.exe")))
            .find(|candidate| is_startable(candidate))
    });

    on_path.or_else(|| {
        fallbacks
            .iter()
            .map(PathBuf::from)
            .find(|candidate| is_startable(candidate))
    })
}

/// Whether `path` is a file this process could start.
#[cfg(windows)]
fn is_startable(path: &Path) -> bool {
    path.is_file() && !is_store_package(path)
}

/// Whether `path` lives under a Windows Store package.
///
/// An executable there is an app-execution alias rather than a program in the
/// filesystem — the elevated account a sandbox runs as cannot start it.
#[cfg(windows)]
fn is_store_package(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str().eq_ignore_ascii_case("WindowsApps"))
}

/// The command interpreter Windows always has.
#[cfg(windows)]
fn comspec() -> PathBuf {
    std::env::var_os("COMSPEC").map_or_else(|| PathBuf::from("cmd.exe"), PathBuf::from)
}

/// The most one command may print, bounding what this process holds while a pipe
/// is still filling.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// The most one command may return to the model.
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// What the model is told the tool does, for the shell it will run in.
///
/// Derived rather than written out, so the interpreter named here is the one
/// this tool actually uses: a model told `/bin/sh` on a machine that runs
/// PowerShell writes the wrong dialect.
fn describe(shell: &Shell) -> String {
    let chain = match shell.dialect {
        Dialect::Posix => "&&",
        Dialect::Windows => ";",
    };

    format!(
        "Run one shell command in the working directory and report what it did. `command` is a \
         single string handed to {label}, so pipes, redirection and {chain} work. The reply is \
         JSON: `stdout`, `stderr`, `exit_code`, and `truncated`. Long output is cut from the \
         front, so the end — where a failure shows — is always there.",
        label = shell.label,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// The command line, as the shell would read it.
    command: String,
}

/// Runs shell commands from the session's working directory.
#[derive(Debug)]
pub struct ShellTool {
    /// How long one command may run before it is stopped.
    timeout: Duration,
    /// The interpreter commands are handed to.
    shell: Shell,
    /// What the model is told, built from `shell` once so `description` can lend
    /// it out rather than compose it per call.
    description: String,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self::new(Duration::from_secs(120))
    }
}

impl ShellTool {
    /// A tool that gives each command `timeout` to finish.
    pub(crate) fn new(timeout: Duration) -> Self {
        let shell = Shell::detect();
        let description = describe(&shell);
        Self {
            timeout,
            shell,
            description,
        }
    }
}

#[async_trait::async_trait]
impl Tool for ShellTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": format!(
                        "The command line to run through {}.",
                        self.shell.label
                    )
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

        let mut child = match builder(ctx, &self.shell, &args.command) {
            Ok(mut command) => match command.spawn() {
                Ok(child) => child,
                Err(err) => {
                    let message = format!("The command did not start: {err}");
                    let hint = spawn_hint(&err, &self.shell);
                    return Ok(ToolOutcome::failure(failure_text(
                        message,
                        Some(hint.as_str()),
                    )));
                }
            },
            Err(err) => {
                let message = format!("The sandbox could not be prepared: {err}");
                let hint = "Nothing was run. A session in this mode needs a working sandbox \
                            runner; switching the session to Full access is the explicit way to \
                            run without one.";
                return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
            }
        };
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        // Drained together while the command runs. Reading one to its end first would
        // leave the child writing into a full pipe, waiting for a reader that is
        // not coming.
        // Run the collection in a task so cancellation can drop it (and the
        // child with it, via kill_on_drop) without borrowing `child` here.
        let handle = tokio::spawn(async move {
            let (out, err) = join!(
                read_capped(stdout, MAX_CAPTURE_BYTES),
                read_capped(stderr, MAX_CAPTURE_BYTES)
            );
            let status = child.wait().await;
            (out, err, status)
        });

        let abort_handle = handle.abort_handle();
        let collected = tokio::select! {
            res = tokio::time::timeout(self.timeout, handle) => res,
            _ = ctx.cancel.cancelled() => {
                abort_handle.abort();
                let message = "Stopped by the user.".to_owned();
                let hint = "The command was interrupted. Whatever it started may still be                             running: the signal reaches the shell, not its children.";
                return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
            }
        };

        let (out, err, status) = match collected {
            Ok(Ok((out, err, status))) => (out, err, status),
            Ok(Err(_)) => {
                let message = "The command's output could not be collected.".to_owned();
                return Ok(ToolOutcome::failure(failure_text(message, None)));
            }
            Err(_) => {
                abort_handle.abort();
                // The signal reaches the shell, not what the shell started: a process
                // tree is not something this holds a handle to.
                let message = format!("Stopped after {:?} without finishing.", self.timeout);
                let hint = "Whatever the command started may still be running: the signal                             reaches the shell, not its children. Check for a leftover process                             before retrying, and split the work into steps that finish inside                             the limit.";
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
fn builder(ctx: &ToolContext, shell: &Shell, command: &str) -> Result<Command, SandboxUnavailable> {
    let mut raw: Vec<OsString> = Vec::with_capacity(shell.args.len() + 2);
    raw.push(shell.program.as_os_str().to_os_string());
    raw.extend(shell.args.iter().map(OsString::from));
    raw.push(OsString::from(command));

    let (program, args) = match &ctx.sandbox {
        SandboxPolicy::Unrestricted => {
            let mut args = raw.into_iter();
            let program = args.next().expect("the shell program is always first");
            (program, args.collect::<Vec<_>>())
        }
        SandboxPolicy::Bounded(policy) => {
            let wrapped = wrap_argv(&raw, policy)?;
            let mut args = wrapped.argv.into_iter();
            let program = args.next().expect("the wrapper always names a program");
            (program, args.collect::<Vec<_>>())
        }
    };

    let mut child = Command::new(program);
    child
        .args(args)
        .current_dir(&ctx.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // An abandoned child would keep running with nobody reading what it
        // writes, and eventually block on a full pipe.
        .kill_on_drop(true);

    // Nothing inherited is kept, so a key the agent has no business reading is
    // not one `env` can print. What is kept is what the interpreter cannot start
    // without.
    child.env_clear();
    keep_environment(&mut child);
    Ok(child)
}

/// Puts back the environment a shell cannot start without, and none of the rest.
///
/// On Unix that is where programs live and where the user's own configuration is
/// read from. On Windows it is the plumbing every program reaches for without
/// being told to: where Windows lives, which interpreter runs a batch file, which
/// extensions count as executable, and where a temporary file may go.
#[cfg(unix)]
fn keep_environment(child: &mut Command) {
    child
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", std::env::var_os("HOME").unwrap_or_default());
}

#[cfg(windows)]
fn keep_environment(child: &mut Command) {
    for name in [
        "PATH",
        "SystemRoot",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERPROFILE",
    ] {
        child.env(name, std::env::var_os(name).unwrap_or_default());
    }
}

/// What to try when the spawn itself failed.
///
/// A spawn that does not happen means the shell is missing or the working
/// directory is gone, and the two call for opposite responses: nothing this tool
/// can fix, or a path the model chose wrong. The shell is named as it was
/// resolved, since on Windows that is whichever of the three layers answered.
fn spawn_hint(err: &std::io::Error, shell: &Shell) -> String {
    if directory_is_gone(err) {
        return format!(
            "Either the working directory does not exist, or {} is not where this expects it. \
             Check the path before running anything else from here.",
            shell.label
        );
    }

    match err.kind() {
        std::io::ErrorKind::PermissionDenied => {
            format!("{} exists but this process may not run it.", shell.label)
        }
        _ => "Nothing about the command was run, so retrying it as written will fail the \
              same way."
            .to_owned(),
    }
}

/// `ERROR_DIRECTORY`: the path named a directory that is not there.
const ERROR_DIRECTORY: i32 = 267;

/// Whether the spawn failed because the working directory is not there.
///
/// Windows answers a directory that has gone with its own code rather than with
/// "not found", so the code is read alongside the kind. No POSIX errno is 267, so
/// the second test is inert on the platforms that never use it.
fn directory_is_gone(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::NotFound || err.raw_os_error() == Some(ERROR_DIRECTORY)
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
        let dir = std::env::temp_dir().join("srud-shell-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    fn ctx(dir: &std::path::Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
            cancel: tokio_util::sync::CancellationToken::new(),
            sandbox: crate::sandbox::SandboxPolicy::Unrestricted,
        }
    }

    /// Runs `command`, expecting well-formed arguments.
    async fn run(dir: &std::path::Path, command: &str) -> ToolOutcome {
        ShellTool::default()
            .call(&ctx(dir), serde_json::json!({ "command": command }))
            .await
            .expect("the arguments match the schema")
    }

    /// The reply, asserting the tool ran the command.
    fn reply(outcome: &ToolOutcome) -> serde_json::Value {
        assert!(!outcome.is_error, "expected a run: {}", outcome.output);
        serde_json::from_str(&outcome.output).expect("the reply is JSON")
    }

    #[tokio::test]
    async fn reports_what_a_command_printed() {
        let dir = scratch("prints");
        let out = reply(&run(&dir, "echo hello").await);

        // The line ending belongs to the shell: a Windows shell ends a line with
        // CRLF, so the assertion is on the text rather than on the bytes ending
        // it.
        assert_eq!(out["stdout"].as_str().expect("stdout").trim_end(), "hello");
        assert_eq!(out["stderr"], "");
        assert_eq!(out["exit_code"], 0);
        assert_eq!(out["truncated"], false);
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[tokio::test]
    async fn does_not_hand_the_command_the_hosts_environment() {
        let dir = scratch("env");
        // Set here, so the assertion does not rest on the developer's own shell.
        std::env::set_var("SRUD_SHELL_TEST_SECRET", "leaked");
        let out = reply(&run(&dir, "echo \"[$SRUD_SHELL_TEST_SECRET]\"").await);
        std::env::remove_var("SRUD_SHELL_TEST_SECRET");

        assert_eq!(
            out["stdout"], "[]\n",
            "the variable did not reach the command"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_a_command_that_outlasts_its_budget() {
        let dir = scratch("timeout");
        let tool = ShellTool::new(Duration::from_millis(150));
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
            .join("srud-shell-tests")
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

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_killed_by_a_signal_reports_no_exit_code() {
        // There is no exit code, so the reply says null rather than a number standing
        // in for one.
        let dir = scratch("signalled");
        let out = reply(&run(&dir, "kill -TERM $$").await);

        assert_eq!(out["exit_code"], serde_json::Value::Null);
    }

    #[cfg(unix)]
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
    async fn an_unknown_argument_is_refused() {
        let dir = scratch("unknown-argument");
        let err = ShellTool::default()
            .call(
                &ctx(&dir),
                serde_json::json!({ "command": "echo hi", "cwd": "/etc" }),
            )
            .await
            .expect_err("unknown fields are refused rather than ignored");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    /// What the resolved interpreter actually does.
    ///
    /// The POSIX tests above spell their commands for a shell Windows does not
    /// start, so they are not run here; these are the same shapes written for
    /// PowerShell, which is what a command goes through on this platform.
    #[cfg(windows)]
    mod windows {
        use super::*;

        /// A resolution that named an absent program would otherwise only fail
        /// in whichever test ran first, with the reason buried in its output.
        #[tokio::test]
        async fn the_resolved_interpreter_runs() {
            let shell = Shell::detect();
            assert!(
                shell.program.is_file(),
                "{} was resolved to {}, which is not a file",
                shell.label,
                shell.program.display()
            );

            let dir = scratch("windows-resolved");
            let out = reply(&run(&dir, "echo started").await);
            assert_eq!(out["exit_code"], 0);
        }

        /// A failing command is the command's own news, not a failed tool call —
        /// including the error stream, which PowerShell writes on its own.
        #[tokio::test]
        async fn a_failing_command_is_reported_rather_than_a_failed_tool() {
            let dir = scratch("windows-failing");
            let outcome = run(&dir, "echo out; [Console]::Error.WriteLine('err'); exit 3").await;
            let out = reply(&outcome);

            assert_eq!(out["stdout"].as_str().expect("stdout").trim_end(), "out");
            assert_eq!(out["stderr"].as_str().expect("stderr").trim_end(), "err");
            assert_eq!(out["exit_code"], 3);
            assert!(!outcome.is_error);
        }

        /// Long output is cut from the front here too, so the end survives.
        #[tokio::test]
        async fn keeps_the_end_of_output_longer_than_the_reply() {
            let dir = scratch("windows-long");
            let command = "1..20000 | ForEach-Object { 'padding' }; 'THE-END'";
            let out = reply(&run(&dir, command).await);
            let stdout = out["stdout"].as_str().expect("stdout");

            assert_eq!(out["truncated"], true);
            assert!(stdout.contains("THE-END"), "the end of the output is kept");
            assert!(
                stdout.len() <= MAX_OUTPUT_BYTES,
                "the reply stays within its budget"
            );
        }

        /// A flood larger than the capture budget still finishes, rather than
        /// blocking on a pipe nobody is reading.
        #[tokio::test]
        async fn output_larger_than_the_capture_budget_does_not_hang() {
            let dir = scratch("windows-flood");
            let command = "1..60000 | ForEach-Object { 'a-fairly-long-line-of-padding' }";
            let outcome = run(&dir, command).await;
            let out = reply(&outcome);

            assert_eq!(out["truncated"], true);
            assert!(!outcome.is_error, "the command finished on its own");
        }

        /// The environment is bare here too: what this process holds is not what
        /// the command can read.
        #[tokio::test]
        async fn does_not_hand_the_command_the_hosts_environment() {
            let dir = scratch("windows-env");
            std::env::set_var("SRUD_SHELL_TEST_SECRET", "leaked");
            let out = reply(&run(&dir, "echo \"[$env:SRUD_SHELL_TEST_SECRET]\"").await);
            std::env::remove_var("SRUD_SHELL_TEST_SECRET");

            assert_eq!(
                out["stdout"].as_str().expect("stdout").trim_end(),
                "[]",
                "the variable did not reach the command"
            );
        }

        /// A command that outlasts its budget is stopped, and the reply says
        /// what was left behind.
        #[tokio::test]
        async fn stops_a_command_that_outlasts_its_budget() {
            let dir = scratch("windows-timeout");
            let tool = ShellTool::new(Duration::from_millis(300));
            let outcome = tool
                .call(
                    &ctx(&dir),
                    serde_json::json!({ "command": "Start-Sleep -Seconds 30" }),
                )
                .await
                .expect("the arguments match the schema");

            assert!(outcome.is_error);
            assert!(
                outcome.output.contains("Stopped after"),
                "the reply says it was stopped: {}",
                outcome.output
            );
        }

        /// A command that stops itself comes back as a result rather than a hung
        /// call.
        ///
        /// Windows has no "killed by a signal": a process that does not finish
        /// ends with whatever code its killer chose, so there is always a number
        /// here — what matters is that it is not one that reads as success.
        #[tokio::test]
        async fn a_command_that_stops_itself_is_reported_rather_than_hanging() {
            let dir = scratch("windows-signalled");
            let out = reply(&run(&dir, "Stop-Process -Id $PID -Force").await);

            assert_ne!(
                out["exit_code"], 0,
                "a command that did not finish does not report success: {out}"
            );
        }

        /// What the model is given: the interpreter this tool will really start,
        /// and no promise of one the machine does not have.
        #[test]
        fn the_description_names_the_interpreter_it_resolved() {
            let tool = ShellTool::default();
            let description = tool.description();

            assert!(
                description.contains(tool.shell.label),
                "the description names {}: {description}",
                tool.shell.label
            );
            assert!(
                !description.contains("/bin/sh"),
                "no POSIX shell is promised on Windows: {description}"
            );
            assert!(
                tool.parameters()["properties"]["command"]["description"]
                    .as_str()
                    .is_some_and(|text| text.contains(tool.shell.label)),
                "the argument is described as going to the same interpreter"
            );
        }

        /// The Store package is where a `bash.exe` that is really the WSL
        /// launcher lives, and where PowerShell cannot be started by the account
        /// a sandbox runs as. Neither is a shell to hand a command to.
        #[test]
        fn a_store_package_is_not_startable() {
            for path in [
                r"C:\Users\user\AppData\Local\Microsoft\WindowsApps\pwsh.exe",
                r"C:\Program Files\WindowsApps\Microsoft.PowerShell_8wekyb3d8bbwe\pwsh.exe",
                r"C:\Users\user\AppData\Local\Microsoft\WindowsApps\bash.exe",
            ] {
                assert!(
                    is_store_package(std::path::Path::new(path)),
                    "{path} is under WindowsApps"
                );
                assert!(!is_startable(std::path::Path::new(path)));
            }

            assert!(!is_store_package(std::path::Path::new(
                r"C:\Program Files\PowerShell\7\pwsh.exe"
            )));
        }
    }
}
