//! The `bash` tool: run one shell command in the session's working directory.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::{join, time::timeout};

use super::{failure_text, Tool, ToolContext, ToolError, ToolOutcome};

/// The name the model calls.
const NAME: &str = "bash";

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

/// Commands refused under a POSIX shell, by the name the shell resolves them to.
///
/// Matched against the command name rather than searched for in the text, so
/// `/bin/rm` and `rm;` are caught while `echo rm` is not. It catches a command
/// that deletes what it names; it does not bound what a command can reach, since
/// redirection (`> file`), `find -delete`, `git clean` and `mv x /dev/null` all
/// destroy files without naming anything here.
const DENIED_POSIX: &[&str] = &[
    "rm", "rmdir", "dd", "mkfs", "fdisk", "shutdown", "reboot", "halt", "poweroff", "chown",
    "chmod",
];

/// Commands refused under PowerShell and cmd, by the name they resolve to.
///
/// Every spelling is listed because PowerShell is case-insensitive and gives the
/// same operation several names: `Remove-Item` also answers to `rm`, `ri`,
/// `del`, `erase`, `rd` and `rmdir`. The POSIX names stay in: PowerShell answers
/// to them as aliases as well, and a POSIX shell may still be reachable from a
/// command line this tool runs.
const DENIED_WINDOWS: &[&str] = &[
    // Deleting what it names, under any of its names.
    "remove-item",
    "ri",
    "rm",
    "rmdir",
    "del",
    "erase",
    "rd",
    // Emptying a file without deleting it.
    "clear-content",
    "clc",
    // Wiping a volume or a whole disk.
    "format",
    "format-volume",
    "diskpart",
    "clear-disk",
    "remove-partition",
    "initialize-disk",
    // Stopping the machine.
    "shutdown",
    "stop-computer",
    "restart-computer",
    "reboot",
    "halt",
    "poweroff",
    // The POSIX names.
    "dd",
    "mkfs",
    "fdisk",
    "chown",
    "chmod",
];

/// The names `dialect` refuses.
fn denied_commands(dialect: Dialect) -> &'static [&'static str] {
    match dialect {
        Dialect::Posix => DENIED_POSIX,
        Dialect::Windows => DENIED_WINDOWS,
    }
}

/// The most one command may print, bounding what this process holds while a pipe
/// is still filling.
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;

/// The most one command may return to the model.
const MAX_OUTPUT_BYTES: usize = 32 * 1024;

/// What the model is told the tool does, for the shell it will run in.
///
/// Derived rather than written out, so the interpreter named here and the
/// commands listed as refused are the ones this tool actually uses: a model told
/// `/bin/sh` on a machine that runs PowerShell writes the wrong dialect, and one
/// told a name the tool does not refuse works around a wall that is not there.
fn describe(shell: &Shell) -> String {
    let refused = denied_commands(shell.dialect)
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    let chain = match shell.dialect {
        Dialect::Posix => "&&",
        Dialect::Windows => ";",
    };

    format!(
        "Run one shell command in the working directory and report what it did. `command` is a \
         single string handed to {label}, so pipes, redirection and {chain} work. The reply is \
         JSON: `stdout`, `stderr`, `exit_code`, and `truncated`. Long output is cut from the \
         front, so the end — where a failure shows — is always there. {refused} are refused; a \
         command that needs one is reported as refused, and spelling it differently will be \
         refused too.",
        label = shell.label,
    )
}

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
    /// The interpreter commands are handed to.
    shell: Shell,
    /// What the model is told, built from `shell` once so `description` can lend
    /// it out rather than compose it per call.
    description: String,
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new(Duration::from_secs(120))
    }
}

impl BashTool {
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
impl Tool for BashTool {
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

        if let Some(name) = refused_command(&args.command, self.shell.dialect) {
            let message = format!("`{name}` is not available through this tool.");
            let hint = "The refusal is on the program name, so another spelling of it, a full \
                        path, or an alias will be refused too. Do not work around it: say that \
                        the task cannot be done this way.";
            return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
        }

        let mut child = match builder(ctx, &self.shell, &args.command).spawn() {
            Ok(child) => child,
            Err(err) => {
                let message = format!("The command did not start: {err}");
                let hint = spawn_hint(&err, &self.shell);
                return Ok(ToolOutcome::failure(failure_text(
                    message,
                    Some(hint.as_str()),
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
fn builder(ctx: &ToolContext, shell: &Shell, command: &str) -> Command {
    let mut child = Command::new(&shell.program);
    child
        .args(shell.args)
        .arg(command)
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
    child
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

/// The first command in `script` that `dialect`'s denials refuse.
fn refused_command(script: &str, dialect: Dialect) -> Option<String> {
    split(script, dialect)
        .iter()
        .filter_map(|segment| segment.split_whitespace().next())
        .filter_map(|token| program_name(token, dialect))
        .find(|name| denied_commands(dialect).contains(&name.as_str()))
}

/// The extensions Windows runs as a program, from `PATHEXT`.
///
/// A name is refused whatever extension it is reached through: `format` is the
/// disk formatter, and the program cmd finds for it is `format.com`.
const PROGRAM_EXTENSIONS: &[&str] = &["exe", "com", "cmd", "bat"];

/// The name a token would resolve to as a program, spelled the way the denials
/// are.
///
/// Whatever directory it was reached through is dropped, and on Windows a
/// trailing extension with it and the case with it: `C:\Windows\System32\DEL.EXE`
/// is the command `del` names, and PowerShell would resolve it just as well
/// without the extension or the capitals.
///
/// Quotes are not part of a name either. `& 'C:\bin\del.exe' x` is how PowerShell
/// runs a path holding a space, and the quoted name is the same deletion as the
/// bare one.
///
/// The Windows side reads the separators itself rather than asking `Path`, which
/// answers differently depending on the platform this was compiled for: a
/// Windows path has to mean the same thing wherever these tests run.
fn program_name(token: &str, dialect: Dialect) -> Option<String> {
    let token = token.trim_matches(['\'', '"']);

    let name = match dialect {
        Dialect::Posix => Path::new(token).file_name()?.to_str()?.to_owned(),
        Dialect::Windows => {
            let mut lowered = token
                .split(['/', '\\'])
                .next_back()
                .unwrap_or_default()
                .to_ascii_lowercase();
            if let Some((stem, extension)) = lowered.rsplit_once('.') {
                if PROGRAM_EXTENSIONS.contains(&extension) {
                    lowered.truncate(stem.len());
                }
            }
            lowered
        }
    };

    (!name.is_empty()).then_some(name)
}

/// Splits a command line into the segments a shell would run separately.
///
/// Separators inside quotes are text, so `echo "a && b"` stays one segment and
/// the `&&` in it cannot start a command that was never written.
///
/// The two dialects cut on the same characters but escape differently: a
/// backslash is PowerShell's path separator, and treating it as an escape would
/// swallow the character after it and run two segments together. cmd is cut by
/// this table too, which is the strict direction — it separates on `;` where cmd
/// would not.
fn split(script: &str, dialect: Dialect) -> Vec<&str> {
    let escape = match dialect {
        Dialect::Posix => '\\',
        Dialect::Windows => '`',
    };
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
                _ if ch == escape => escaped = true,
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

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[tokio::test]
    async fn stops_a_command_that_outlasts_its_budget() {
        let dir = scratch("timeout");
        let tool = BashTool::new(Duration::from_millis(150));
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

    /// A shell of `dialect`, for the tests that ask what a command line means
    /// without starting one.
    fn shell(dialect: Dialect) -> Shell {
        Shell {
            program: PathBuf::from("shell"),
            args: &["-c"],
            dialect,
            label: "shell",
        }
    }

    #[test]
    fn refuses_the_commands_it_lists() {
        // A command the description names and the tool does not refuse is one the model
        // tries, and then works around.
        for dialect in [Dialect::Posix, Dialect::Windows] {
            let description = describe(&shell(dialect));
            for name in denied_commands(dialect) {
                let script = refused_command(&format!("{name} --help"), dialect);
                assert_eq!(
                    script.as_deref(),
                    Some(*name),
                    "{name} is listed but not refused under {dialect:?}"
                );
                assert!(
                    description.contains(name),
                    "{name} is refused but not named in the description"
                );
            }
        }
    }

    #[test]
    fn finds_a_denied_command_wherever_it_appears_in_a_posix_line() {
        assert_eq!(
            refused_command("rm x", Dialect::Posix).as_deref(),
            Some("rm")
        );
        assert_eq!(
            refused_command("/usr/bin/rm x", Dialect::Posix).as_deref(),
            Some("rm")
        );
        assert_eq!(
            refused_command("a | b ; rm", Dialect::Posix).as_deref(),
            Some("rm")
        );
        assert_eq!(
            refused_command("a && rm && b", Dialect::Posix).as_deref(),
            Some("rm")
        );
        assert_eq!(
            refused_command("(rm)", Dialect::Posix).as_deref(),
            Some("rm")
        );
        assert_eq!(
            refused_command("'/bin/rm' x", Dialect::Posix).as_deref(),
            Some("rm")
        );
        assert_eq!(refused_command("echo rm", Dialect::Posix).as_deref(), None);
        assert_eq!(
            refused_command("echo 'a; rm'", Dialect::Posix).as_deref(),
            None
        );
        assert_eq!(refused_command("", Dialect::Posix).as_deref(), None);
    }

    /// The Windows side of the same reading: a backslash is a path separator
    /// rather than an escape, and a name is matched without its case, its
    /// extension and its quotes.
    #[test]
    fn finds_a_denied_command_wherever_it_appears_in_a_windows_line() {
        assert_eq!(
            refused_command("Remove-Item x", Dialect::Windows).as_deref(),
            Some("remove-item")
        );
        assert_eq!(
            refused_command("Get-ChildItem . | Remove-Item -Force", Dialect::Windows).as_deref(),
            Some("remove-item")
        );
        assert_eq!(
            refused_command("Write-Host hi; DEL /F x", Dialect::Windows).as_deref(),
            Some("del")
        );
        assert_eq!(
            refused_command(r"C:\Windows\System32\del.exe x", Dialect::Windows).as_deref(),
            Some("del")
        );
        assert_eq!(
            refused_command(r#"& "C:\Windows\System32\DEL.EXE" x"#, Dialect::Windows).as_deref(),
            Some("del")
        );
        assert_eq!(
            refused_command("'Remove-Item' note.txt", Dialect::Windows).as_deref(),
            Some("remove-item")
        );
        // The extension is one cmd resolves a program through, not only `.exe`.
        assert_eq!(
            refused_command("format.com C:", Dialect::Windows).as_deref(),
            Some("format")
        );
        // A backslash is not an escape here, so the command after it is still a
        // command rather than the tail of the one before.
        assert_eq!(
            refused_command(r"echo C:\temp; del x", Dialect::Windows).as_deref(),
            Some("del")
        );
        assert_eq!(
            refused_command("echo del", Dialect::Windows).as_deref(),
            None
        );
        assert_eq!(
            refused_command("echo 'del x'", Dialect::Windows).as_deref(),
            None
        );
        assert_eq!(
            refused_command("Write-Host \"Remove-Item is refused\"", Dialect::Windows).as_deref(),
            None
        );
        assert_eq!(refused_command("", Dialect::Windows).as_deref(), None);
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
            std::env::set_var("SRUD_BASH_TEST_SECRET", "leaked");
            let out = reply(&run(&dir, "echo \"[$env:SRUD_BASH_TEST_SECRET]\"").await);
            std::env::remove_var("SRUD_BASH_TEST_SECRET");

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
            let tool = BashTool::new(Duration::from_millis(300));
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

        /// Every spelling PowerShell resolves to deletion is refused before
        /// anything runs: the cmdlets, their aliases, and the same names through
        /// a `.exe` or a path.
        #[tokio::test]
        async fn refuses_the_deletions_powershell_answers_to() {
            for command in [
                "Remove-Item note.txt",
                "remove-item note.txt",
                "Remove-Item -Path note.txt -Recurse -Force",
                "ri note.txt",
                "del note.txt",
                "Erase note.txt",
                "rd note.txt",
                "del.exe note.txt",
                r"C:\Windows\System32\del.exe note.txt",
                r#"& 'C:\Windows\System32\del.exe' note.txt"#,
                "Write-Host hi; Remove-Item note.txt",
                "Get-ChildItem . | Remove-Item",
            ] {
                let dir = scratch("windows-refused");
                std::fs::write(dir.join("note.txt"), "still here").expect("fixture file");
                let message = refusal(&run(&dir, command).await);

                assert!(
                    message.contains("not available"),
                    "the refusal is reported: {message} ({command})"
                );
                assert!(
                    dir.join("note.txt").exists(),
                    "the file is still there after {command}"
                );
            }
        }

        /// Text that merely contains a refused name is not a command.
        #[tokio::test]
        async fn leaves_text_that_only_looks_like_a_refused_command() {
            for command in [
                "Write-Host del",
                "Write-Host 'Remove-Item note.txt'",
                "Write-Output 'rd /s /q'",
            ] {
                let dir = scratch("windows-allowed");
                let outcome = run(&dir, command).await;
                assert!(
                    !outcome.is_error,
                    "{command} should have run: {}",
                    outcome.output
                );
            }
        }

        /// What the model is given: the interpreter this tool will really start,
        /// and no promise of one the machine does not have.
        #[test]
        fn the_description_names_the_interpreter_it_resolved() {
            let tool = BashTool::default();
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
