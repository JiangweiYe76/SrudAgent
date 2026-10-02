//! The `read` tool: a window onto one file.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, BufReader};

use super::{Tool, ToolContext, ToolError, ToolOutcome};

/// The name the model calls.
const NAME: &str = "read";

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Read a file. `path` may be absolute or relative to the working directory. \
`offset` is the 1-indexed line to start at and `limit` caps how many lines \
follow. The reply is JSON: the resolved path, the first and last 1-indexed \
line numbers it covers, the content, and `truncated`, which says whether the \
file continues after the last line given. Lines come back whole; to keep \
reading, ask again with `offset` set to the last line number plus one.";

/// The most one call returns, so a single file cannot flood the context.
const MAX_BYTES: usize = 64 * 1024;

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// Absolute, or relative to the working directory.
    path: String,
    /// The 1-indexed line to start at. Defaults to the first.
    offset: Option<usize>,
    /// How many lines to return. Defaults to the rest of the file.
    limit: Option<usize>,
}

/// Reads files from the session's working directory.
pub struct ReadTool;

#[async_trait::async_trait]
impl Tool for ReadTool {
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
                "path": {
                    "type": "string",
                    "description": "Absolute, or relative to the working directory."
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "The 1-indexed line to start at. Defaults to 1."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "How many lines to return. Defaults to the rest of the file."
                }
            },
            "required": ["path"],
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
        let offset = args.offset.unwrap_or(1);
        if offset == 0 {
            return Err(invalid_arguments("offset must be a 1-indexed line number"));
        }
        if args.limit == Some(0) {
            return Err(invalid_arguments("limit must be a positive line count"));
        }

        let path = resolve(ctx, &args.path);
        let shown = path.display();

        match tokio::fs::metadata(&path).await {
            Ok(metadata) if !metadata.is_file() => {
                return Ok(ToolOutcome::failure(format!("{shown} is not a file")));
            }
            Ok(_) => {}
            Err(err) => {
                return Ok(ToolOutcome::failure(format!("cannot open {shown}: {err}")));
            }
        }

        let file = match File::open(&path).await {
            Ok(file) => file,
            Err(err) => return Ok(ToolOutcome::failure(format!("cannot open {shown}: {err}"))),
        };

        let window = match read_window(file, offset, args.limit).await {
            Ok(window) => window,
            Err(ScanError::NoSuchLine(line)) => {
                return Ok(ToolOutcome::failure(format!("{shown} has no line {line}")));
            }
            Err(ScanError::LineTooLong(line)) => {
                return Ok(ToolOutcome::failure(format!(
                    "line {line} of {shown} is longer than the {MAX_BYTES}-byte read budget, \
                     so it cannot come back whole"
                )));
            }
            Err(ScanError::NotUtf8(line)) => {
                return Ok(ToolOutcome::failure(format!(
                    "{shown} is not UTF-8 text (line {line})"
                )));
            }
            Err(ScanError::Io(err)) => {
                return Ok(ToolOutcome::failure(format!("cannot read {shown}: {err}")));
            }
        };

        let reply = serde_json::json!({
            "path": path.display().to_string(),
            "start_line_number": offset,
            "end_line_number": window.end_line,
            "content": window.content,
            "truncated": window.truncated,
        });
        Ok(ToolOutcome::success(reply.to_string()))
    }
}

/// The slice of a file one call returns.
struct Window {
    content: String,
    /// The 1-indexed number of the last line in `content`, or `offset - 1` when
    /// no line was returned. Reading on means asking for `end_line + 1`.
    end_line: usize,
    /// Whether the file holds anything after `end_line`.
    truncated: bool,
}

/// What stopped a scan.
enum ScanError {
    /// The file does not reach the requested line.
    NoSuchLine(usize),
    /// One line is larger than the whole budget, so it cannot come back whole.
    LineTooLong(usize),
    /// The bytes read are not UTF-8 text.
    NotUtf8(usize),
    /// The file could not be read.
    Io(std::io::Error),
}

impl From<std::io::Error> for ScanError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err)
    }
}

/// Reads the requested window without holding the file.
///
/// Bytes are streamed through a fixed buffer: reaching a line deep in a large
/// file costs no memory, and only the window itself is kept.
async fn read_window(file: File, offset: usize, limit: Option<usize>) -> Result<Window, ScanError> {
    let mut reader = BufReader::new(file);
    skim_to(&mut reader, offset).await?;

    let mut content = String::new();
    // The bytes of the line being assembled; a line is added to `content` only
    // once its newline has arrived.
    let mut pending: Vec<u8> = Vec::new();
    let mut lines_taken = 0usize;

    loop {
        if limit.is_some_and(|limit| lines_taken == limit) {
            // Asked once, so `truncated` is a fact about the file rather than a
            // guess that would send the caller to a line that does not exist.
            let more = !reader.fill_buf().await?.is_empty();
            return Ok(window(content, offset, lines_taken, more));
        }

        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            // The file ended without a trailing newline. What is buffered is
            // still a line, and dropping it would hide the end of the file.
            if !pending.is_empty() {
                take_line(&mut content, &pending, offset + lines_taken)?;
                lines_taken += 1;
            }
            return Ok(window(content, offset, lines_taken, false));
        }

        let take = match chunk.iter().position(|&byte| byte == b'\n') {
            Some(index) => index + 1,
            None => chunk.len(),
        };
        // A line is taken whole or not at all: half a line misleads, and cannot
        // be continued from either.
        if content.len() + pending.len() + take > MAX_BYTES {
            return if lines_taken == 0 {
                Err(ScanError::LineTooLong(offset))
            } else {
                Ok(window(content, offset, lines_taken, true))
            };
        }

        pending.extend_from_slice(&chunk[..take]);
        reader.consume(take);

        if pending.last() == Some(&b'\n') {
            take_line(&mut content, &pending, offset + lines_taken)?;
            pending.clear();
            lines_taken += 1;
        }
    }
}

/// Appends one line, rejecting bytes that are not UTF-8 text.
fn take_line(content: &mut String, line: &[u8], line_number: usize) -> Result<(), ScanError> {
    let text = std::str::from_utf8(line).map_err(|_| ScanError::NotUtf8(line_number))?;
    content.push_str(text);
    Ok(())
}

/// Pairs the lines collected so far with the number of the last one.
fn window(content: String, offset: usize, lines_taken: usize, truncated: bool) -> Window {
    Window {
        content,
        // `offset` is at least 1, so an empty window lands on the line before it.
        end_line: offset - 1 + lines_taken,
        truncated,
    }
}

/// Reads past the lines before `offset`, keeping none of them.
async fn skim_to(reader: &mut BufReader<File>, offset: usize) -> Result<(), ScanError> {
    let mut line = 1;
    while line < offset {
        let chunk = reader.fill_buf().await?;
        if chunk.is_empty() {
            return Err(ScanError::NoSuchLine(offset));
        }
        let take = match chunk.iter().position(|&byte| byte == b'\n') {
            Some(index) => {
                line += 1;
                index + 1
            }
            None => chunk.len(),
        };
        reader.consume(take);
    }
    Ok(())
}

/// Resolves the requested path: an absolute one is taken as given, a relative
/// one is anchored to the working directory.
fn resolve(ctx: &ToolContext, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        ctx.cwd.join(path)
    }
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
    use super::*;

    /// A directory for one test to read from, emptied first so a rerun starts
    /// from the same place.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("srud-read-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    /// A scratch directory holding one file.
    fn scratch_with(name: &str, file: &str, contents: &str) -> PathBuf {
        let dir = scratch(name);
        std::fs::write(dir.join(file), contents).expect("fixture file");
        dir
    }

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
        }
    }

    /// Runs the tool against `dir`, expecting well-formed arguments.
    async fn read(dir: &Path, arguments: serde_json::Value) -> ToolOutcome {
        ReadTool
            .call(&ctx(dir), arguments)
            .await
            .expect("the arguments match the schema")
    }

    /// The reply, asserting the tool reported success.
    fn reply(outcome: &ToolOutcome) -> serde_json::Value {
        assert!(!outcome.is_error, "expected success: {}", outcome.output);
        serde_json::from_str(&outcome.output).expect("the reply is JSON")
    }

    /// A file of `lines` lines, each `width` characters wide.
    fn block(lines: usize, width: usize) -> String {
        vec!["x".repeat(width); lines].join("\n")
    }

    #[tokio::test]
    async fn reads_a_whole_file() {
        let dir = scratch_with("whole", "note.txt", "one\ntwo\nthree\n");
        let out = reply(&read(&dir, serde_json::json!({ "path": "note.txt" })).await);

        assert_eq!(out["content"], "one\ntwo\nthree\n");
        assert_eq!(out["start_line_number"], 1);
        assert_eq!(out["end_line_number"], 3);
        assert_eq!(out["truncated"], false);
        let expected = dir.join("note.txt").display().to_string();
        assert_eq!(
            out["path"].as_str(),
            Some(expected.as_str()),
            "the reply names the file it actually read"
        );
    }

    #[tokio::test]
    async fn offset_starts_where_the_caller_asked() {
        let dir = scratch_with("offset", "note.txt", "one\ntwo\nthree\n");
        let out = reply(&read(&dir, serde_json::json!({ "path": "note.txt", "offset": 2 })).await);

        assert_eq!(out["content"], "two\nthree\n");
        assert_eq!(out["start_line_number"], 2);
        assert_eq!(out["end_line_number"], 3);
        assert_eq!(out["truncated"], false, "the offset reached the end");
    }

    #[tokio::test]
    async fn limit_caps_how_many_lines_come_back() {
        let dir = scratch_with("limit", "note.txt", "one\ntwo\nthree\n");
        let out = reply(&read(&dir, serde_json::json!({ "path": "note.txt", "limit": 2 })).await);

        assert_eq!(out["content"], "one\ntwo\n");
        assert_eq!(out["end_line_number"], 2);
        assert_eq!(out["truncated"], true, "there is a line after this window");
    }

    #[tokio::test]
    async fn a_limit_that_reaches_the_end_is_not_truncated() {
        let dir = scratch_with("limit-at-end", "note.txt", "one\ntwo\n");
        let out = reply(&read(&dir, serde_json::json!({ "path": "note.txt", "limit": 2 })).await);

        assert_eq!(out["end_line_number"], 2);
        assert_eq!(
            out["truncated"], false,
            "the caller sent a limit, but the file ends here all the same"
        );
    }

    #[tokio::test]
    async fn a_deep_offset_skips_the_lines_before_it() {
        let dir = scratch_with("deep-offset", "big.txt", &block(20_000, 40));
        let out = reply(
            &read(
                &dir,
                serde_json::json!({ "path": "big.txt", "offset": 19_000 }),
            )
            .await,
        );

        assert_eq!(out["start_line_number"], 19_000);
        assert_eq!(out["end_line_number"], 20_000);
        assert_eq!(out["truncated"], false);
        assert_eq!(
            out["content"].as_str().expect("content").lines().count(),
            1001,
            "the window runs from the offset to the end of the file"
        );
    }

    #[tokio::test]
    async fn offset_and_limit_take_a_window_of_the_file() {
        let dir = scratch_with("window", "note.txt", "one\ntwo\nthree\nfour\n");
        let out = reply(
            &read(
                &dir,
                serde_json::json!({ "path": "note.txt", "offset": 2, "limit": 2 }),
            )
            .await,
        );

        assert_eq!(out["content"], "two\nthree\n");
        assert_eq!(out["start_line_number"], 2);
        assert_eq!(out["end_line_number"], 3);
        assert_eq!(out["truncated"], true);
    }

    #[tokio::test]
    async fn reading_on_from_the_end_line_reaches_the_end_of_the_file() {
        let contents = block(200, 1024);
        let dir = scratch_with("continuation", "big.txt", &contents);

        // The first window cannot hold the whole file, so it stops on a line
        // boundary and says where it stopped.
        let first = reply(&read(&dir, serde_json::json!({ "path": "big.txt" })).await);
        assert_eq!(first["truncated"], true);
        let first_end = first["end_line_number"].as_u64().expect("a line number");
        let mut collected = first["content"].as_str().expect("content").to_owned();

        // Asking for the next line must pick up exactly where it stopped: no
        // gap, no overlap, no repeated text.
        let second = reply(
            &read(
                &dir,
                serde_json::json!({ "path": "big.txt", "offset": first_end + 1 }),
            )
            .await,
        );
        collected.push_str(second["content"].as_str().expect("content"));

        assert!(
            contents.starts_with(&collected),
            "the windows are a prefix of the file: {} bytes collected",
            collected.len()
        );
        assert!(
            collected.len() > MAX_BYTES,
            "two windows carry more than one"
        );
    }

    #[tokio::test]
    async fn an_absolute_path_is_taken_as_given() {
        let dir = scratch_with("absolute", "note.txt", "one\n");
        let path = dir.join("note.txt");
        let out = reply(&read(&dir, serde_json::json!({ "path": path })).await);

        assert_eq!(out["content"], "one\n");
    }

    #[tokio::test]
    async fn a_last_line_without_a_newline_still_comes_back() {
        let dir = scratch_with("no-trailing-newline", "note.txt", "one\ntwo");
        let out = reply(&read(&dir, serde_json::json!({ "path": "note.txt" })).await);

        assert_eq!(out["content"], "one\ntwo");
        assert_eq!(out["end_line_number"], 2, "the file holds two lines");
        assert_eq!(out["truncated"], false);
    }

    #[tokio::test]
    async fn a_zero_offset_is_a_bad_argument() {
        let dir = scratch("zero-offset");
        let err = ReadTool
            .call(
                &ctx(&dir),
                serde_json::json!({ "path": "note.txt", "offset": 0 }),
            )
            .await
            .expect_err("lines are 1-indexed");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn a_zero_limit_is_a_bad_argument() {
        let dir = scratch("zero-limit");
        let err = ReadTool
            .call(
                &ctx(&dir),
                serde_json::json!({ "path": "note.txt", "limit": 0 }),
            )
            .await
            .expect_err("a limit of zero reads nothing");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn an_unknown_argument_is_refused() {
        let dir = scratch("unknown-argument");
        let err = ReadTool
            .call(
                &ctx(&dir),
                serde_json::json!({ "path": "note.txt", "encoding": "utf-8" }),
            )
            .await
            .expect_err("unknown fields are refused rather than ignored");
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[tokio::test]
    async fn a_missing_file_is_a_failed_outcome() {
        let dir = scratch("missing");
        let outcome = read(&dir, serde_json::json!({ "path": "nope.txt" })).await;

        assert!(outcome.is_error, "a missing file is not a runtime error");
        assert!(
            outcome.output.contains("nope.txt"),
            "the failure names the file: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn a_directory_is_a_failed_outcome() {
        let dir = scratch("directory");
        let outcome = read(&dir, serde_json::json!({ "path": "." })).await;

        assert!(outcome.is_error);
        assert!(
            outcome.output.contains("not a file"),
            "a directory is refused clearly: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn an_offset_past_the_end_is_a_failed_outcome() {
        let dir = scratch_with("past-end", "note.txt", "one\n");
        let outcome = read(&dir, serde_json::json!({ "path": "note.txt", "offset": 9 })).await;

        assert!(outcome.is_error);
        assert!(
            outcome.output.contains("line 9"),
            "the failure names the line: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn an_empty_file_yields_nothing_and_says_so() {
        let dir = scratch_with("empty", "empty.txt", "");
        let out = reply(&read(&dir, serde_json::json!({ "path": "empty.txt" })).await);

        assert_eq!(out["content"], "");
        assert_eq!(out["end_line_number"], 0, "no line was returned");
        assert_eq!(out["truncated"], false);
    }

    #[tokio::test]
    async fn a_long_file_is_cut_on_a_line_boundary_within_the_budget() {
        let dir = scratch_with("budget", "big.txt", &block(200, 1024));
        let out = reply(&read(&dir, serde_json::json!({ "path": "big.txt" })).await);

        assert_eq!(out["truncated"], true);
        let content = out["content"].as_str().expect("content is a string");
        assert!(content.len() <= MAX_BYTES, "{} bytes", content.len());
        assert!(
            content.ends_with('\n'),
            "the window stops between lines, not inside one"
        );
        let end = out["end_line_number"].as_u64().expect("a line number");
        assert_eq!(
            content.lines().count() as u64,
            end,
            "the last line number is the number of lines returned"
        );
    }

    #[tokio::test]
    async fn a_line_longer_than_the_budget_is_refused() {
        // One line, twice the budget: it can never come back whole, and a
        // half-line would be both misleading and impossible to continue from.
        let dir = scratch_with("long-line", "min.js", &"é".repeat(MAX_BYTES));
        let outcome = read(&dir, serde_json::json!({ "path": "min.js" })).await;

        assert!(outcome.is_error);
        assert!(
            outcome.output.contains("line 1") && outcome.output.contains("longer than"),
            "the failure says the line itself is the problem: {}",
            outcome.output
        );
    }

    #[tokio::test]
    async fn a_long_line_that_fits_comes_back_whole() {
        let line = format!("{}\n", "é".repeat(1000));
        let dir = scratch_with("multibyte", "wide.txt", &line);
        let out = reply(&read(&dir, serde_json::json!({ "path": "wide.txt" })).await);

        assert_eq!(out["content"], line.as_str());
        assert_eq!(out["end_line_number"], 1);
        assert_eq!(out["truncated"], false);
    }
}
