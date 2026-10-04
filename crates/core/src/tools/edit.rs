//! The `edit` tool: exact text replaced in one file.

use std::ops::Range;
use std::path::Path;

use serde::Deserialize;
use tokio::io::AsyncReadExt;

use super::{
    failure_text, invalid_arguments, resolve, LenientBool, Tool, ToolContext, ToolError,
    ToolOutcome,
};

/// The name the model calls.
const NAME: &str = "edit";

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Replace exact text in a file. `path` may be absolute or relative to the working \
directory, and each entry in `edits` is one replacement: `old_string` is text \
that must appear in the file exactly as written, and `new_string` takes its \
place. An empty `new_string` deletes what was matched. Every `old_string` is \
matched against the file as it is now rather than against the result of the \
other edits, so one replacement cannot move the target of the next — but two \
replacements must not overlap. An `old_string` that appears more than once is \
refused, because there is no telling which one was meant; set `replace_all` to \
change every occurrence, and only when every one of them should change. The \
reply is JSON: the resolved path, how many replacements were made, and `diff`, \
which shows each replaced text removed and its replacement added. When \
`old_string` is not in the file the reply says so and shows how the file begins; \
when it is there more than once it says how many and which lines they are on, so \
reading the file and quoting more of it is enough to make the next call succeed.";

/// The most matches a refusal will point at, so a file with fifty of them does
/// not bury the reason in a wall of context.
const MAX_REPORTED_MATCHES: usize = 2;

/// How much of a file a refusal quotes when the text is nowhere in it.
const PREVIEW_LINES: usize = 20;

/// The byte ceiling on that quote, for the files whose lines are long.
const PREVIEW_BYTES: usize = 2 * 1024;

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// Absolute, or relative to the working directory.
    path: String,
    /// The replacements to make. An empty list is refused: it asks for a round
    /// trip and changes nothing.
    edits: Vec<Edit>,
    /// Whether an `old_string` occurring more than once changes every occurrence.
    replace_all: Option<LenientBool>,
}

/// One replacement.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    /// Text that must appear exactly as written.
    old_string: String,
    /// What takes its place. Empty deletes the match.
    new_string: String,
}

/// Edits text in the session's working directory.
pub struct EditTool;

#[async_trait::async_trait]
impl Tool for EditTool {
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
                "edits": {
                    "type": "array",
                    "minItems": 1,
                    "description": "The replacements to make. Each `old_string` is matched \
                                    against the file as it is now, not against the result of \
                                    the other edits, and no two may overlap.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": {
                                "type": "string",
                                "description": "Text that must appear in the file exactly as \
                                                written, whitespace and indentation included. Must \
                                                occur exactly once unless `replace_all` is true. \
                                                Must not be empty."
                            },
                            "new_string": {
                                "type": "string",
                                "description": "What takes its place. An empty string deletes \
                                                the matched text."
                            }
                        },
                        "required": ["old_string", "new_string"],
                        "additionalProperties": false
                    }
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Change every occurrence of an `old_string` that is not \
                                    unique. Defaults to false, which refuses instead."
                }
            },
            "required": ["path", "edits"],
            "additionalProperties": false
        })
    }

    fn validate(&self, arguments: &serde_json::Value) -> Result<(), String> {
        let mut problems: Vec<String> = Vec::new();
        match arguments.get("edits").and_then(serde_json::Value::as_array) {
            None => {
                problems.push("/edits: is required and must be a list of replacements".to_owned())
            }
            Some(list) if list.is_empty() => {
                problems.push(
                    "/edits: must hold at least one replacement, or nothing changes".to_owned(),
                );
            }
            Some(list) => problems.extend(unusable_edits(list)),
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("; "))
        }
    }

    async fn call(
        &self,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let args: Args = serde_json::from_value(arguments)
            .map_err(|err| invalid_arguments(NAME, err.to_string()))?;
        let replace_all = args.replace_all.is_some_and(|flag| flag.0);

        let path = resolve(ctx, &args.path);
        let shown = path.display().to_string();

        let content = match read_source(&path).await {
            Ok(content) => content,
            Err(message) => return Ok(ToolOutcome::failure(message)),
        };
        // Converted before anything is compared, so the guard below sees what the
        // two strings will actually be once they are in the file's own spelling.
        let ending = uniform_line_ending(&content);
        let edits: Vec<Edit> = args
            .edits
            .into_iter()
            .map(|edit| Edit {
                old_string: to_line_ending(&edit.old_string, ending),
                new_string: to_line_ending(&edit.new_string, ending),
            })
            .collect();
        if let Err(message) = same_after_conversion(&edits) {
            return Ok(ToolOutcome::failure(failure_text(message, Some(EDIT_HINT))));
        }

        let mut planned = Vec::new();
        for (index, edit) in edits.iter().enumerate() {
            match plan(&content, edit, replace_all, &shown) {
                Ok(ranges) => planned.push(Planned {
                    ranges,
                    text: edit.new_string.as_str(),
                    old: edit.old_string.as_str(),
                }),
                Err(message) => {
                    // Named, because a call can carry several and a refusal that
                    // quotes the text without saying which one it was leaves the
                    // model to diff it against what it sent.
                    let message = format!("/edits/{index}: {message}");
                    return Ok(ToolOutcome::failure(failure_text(message, Some(EDIT_HINT))));
                }
            }
        }
        if let Err(message) = no_overlap(&content, &planned) {
            // No hint: both of its remedies are about text that is missing or
            // repeated, and this refusal is about two replacements sharing it.
            return Ok(ToolOutcome::failure(failure_text(message, None)));
        }

        let replacements: usize = planned.iter().map(|edit| edit.ranges.len()).sum();
        let mut next = content.clone();
        let mut splices: Vec<(Range<usize>, &str)> = planned
            .iter()
            .flat_map(|edit| {
                edit.ranges
                    .iter()
                    .map(move |range| (range.clone(), edit.text))
            })
            .collect();
        // Ascending, then walked from the back. Each splice is therefore at a
        // lower offset than every one already made, so replacing it cannot shift
        // an offset that is still to be used.
        splices.sort_by_key(|(range, _)| range.start);
        for (range, text) in splices.into_iter().rev() {
            next.replace_range(range, text);
        }

        if let Err(err) = tokio::fs::write(&path, next.as_bytes()).await {
            return Ok(ToolOutcome::failure(write_failure(&path, &err)));
        }

        let reply = serde_json::json!({
            "path": path.display().to_string(),
            "replacements": replacements,
            "diff": preview(&planned),
        });
        let touched = path.display().to_string();
        Ok(ToolOutcome::success(reply.to_string()).with_paths([touched]))
    }
}

/// Appended to a refusal about text that is missing or repeated, so that each
/// ends with something to try.
///
/// The two remedies cover the two ways those go wrong. A refusal about
/// overlapping replacements carries none of it, because it is about neither.
const EDIT_HINT: &str = "Nothing was changed. Read the file, then quote the text to replace \
                          exactly as it appears — whitespace, indentation and line endings \
                          included — with enough lines around it to identify one spot, or set \
                          `replace_all` if every occurrence should change.";

/// One replacement located in the file.
struct Planned<'a> {
    /// Where the matched text starts and ends, ascending and non-overlapping.
    ranges: Vec<Range<usize>>,
    /// What goes there.
    text: &'a str,
    /// What was there, kept so the reply can show what it displaced.
    old: &'a str,
}

/// The replacements that cannot be run at all, named by their argument path.
///
/// Reported together so one round trip fixes them all. Run before the file is
/// read, which is why these are refusals of the arguments rather than of the
/// file: a call that names nothing to match cannot be answered by any file.
fn unusable_edits(list: &[serde_json::Value]) -> Vec<String> {
    let mut problems = Vec::new();
    for (index, edit) in list.iter().enumerate() {
        let old = edit.get("old_string").and_then(serde_json::Value::as_str);
        let new = edit.get("new_string").and_then(serde_json::Value::as_str);
        match (old, new) {
            (None, _) => problems.push(format!("/edits/{index}/old_string: is required")),
            (Some(_), None) => {
                problems.push(format!("/edits/{index}/new_string: is required"));
            }
            (Some(""), _) => problems.push(format!(
                "/edits/{index}/old_string: must not be empty; there is no text to match, and \
                 `write` replaces a file's whole contents"
            )),
            (Some(old), Some(new)) if old == new => problems.push(format!(
                "/edits/{index}: old_string and new_string are the same, so this changes nothing"
            )),
            _ => {}
        }
    }
    problems
}

/// Locates one replacement in the file, refusing the ones that cannot be pinned
/// down.
fn plan(
    content: &str,
    edit: &Edit,
    replace_all: bool,
    shown: &str,
) -> Result<Vec<Range<usize>>, String> {
    let ranges = non_overlapping_ranges(content, &edit.old_string);
    match ranges.len() {
        0 => Err(format!(
            "Nothing in {shown} matches the text to replace.\n\nLooking for:\n```\n{}\n```\
             \n\nThe file begins:\n```\n{}\n```",
            quote(&edit.old_string),
            quote(&opening_lines(content))
        )),
        1 => Ok(ranges),
        count => {
            if replace_all {
                Ok(ranges)
            } else {
                Err(ambiguous(&edit.old_string, content, &ranges, count))
            }
        }
    }
}

/// Where `needle` occurs in `content`, counted the way a replacement applies them.
///
/// Left to right and never overlapping itself: `aa` occurs twice in `aaa` under a
/// count that lets matches share a byte and once under this one. The shared-byte
/// reading has no application — the two matches cannot both be replaced, because
/// the second one is inside the text the first one removes — so counting it would
/// claim a `replace_all` that cannot be carried out.
fn non_overlapping_ranges(content: &str, needle: &str) -> Vec<Range<usize>> {
    content
        .match_indices(needle)
        .map(|(at, _)| at..at + needle.len())
        .collect()
}

/// The refusal for text that occurs more than once, pointing at where.
///
/// The line numbers are the point: with them the next call is "read those lines,
/// quote more around one of them" rather than a guess.
fn ambiguous(old: &str, content: &str, ranges: &[Range<usize>], count: usize) -> String {
    let mut message = format!(
        "Found {count} matches of the text to replace, so there is no telling which one was \
         meant. Give more surrounding lines to identify one of them, or set `replace_all` to \
         change all {count}.\n\nLooking for:\n```\n{}\n```\n\n",
        quote(old)
    );
    for (index, range) in ranges.iter().take(MAX_REPORTED_MATCHES).enumerate() {
        let line = line_of(content, range.start);
        message.push_str(&format!(
            "Match {} at line {line}:\n```\n{}\n```\n\n",
            index + 1,
            quote(line_text(content, line))
        ));
    }
    if count > MAX_REPORTED_MATCHES {
        message.push_str(&format!("...and {} more.\n", count - MAX_REPORTED_MATCHES));
    }
    message.trim_end().to_owned()
}

/// The refusal for two replacements that want some of the same text.
///
/// Overlapping is refused rather than resolved by order: which of the two wins is
/// a decision the caller did not make, and the other would then be matching text
/// the caller never saw.
fn no_overlap(content: &str, planned: &[Planned<'_>]) -> Result<(), String> {
    let mut hits: Vec<(Range<usize>, &str)> = planned
        .iter()
        .flat_map(|edit| {
            edit.ranges
                .iter()
                .map(move |range| (range.clone(), edit.old))
        })
        .collect();
    hits.sort_by_key(|(range, _)| range.start);
    for pair in hits.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        if first.0.end <= second.0.start {
            continue;
        }
        // The same text twice is a different mistake from two texts meeting, and
        // naming it as an overlap would send the caller off narrowing two edits
        // when the answer is to send one.
        let first_line = line_of(content, first.0.start);
        let second_line = line_of(content, second.0.start);
        return Err(if first.1 == second.1 {
            format!(
                "This call asks for the same text to be replaced twice:\n```\n{}\n```\n\
                 It appears at line {first_line}, and both replacements would land there. \
                 Send one replacement for it; if the two meant different changes, quote \
                 more context so they name different text.",
                quote(first.1)
            )
        } else {
            format!(
                "Two of the replacements in this call overlap: the text\n```\n{}\n```\n\
                 is inside a replacement that starts on line {}, and would be replaced again \
                 from line {second_line}. Merge them into one replacement, or narrow each so \
                 they touch different text.",
                quote(first.1),
                first_line
            )
        });
    }
    Ok(())
}

/// The reply's `diff`: each replaced text as removed and its replacement as
/// added.
///
/// A summary of the call's own arguments rather than a diff of the file, so it
/// reads as what the model asked for having been done. A replacement that
/// covered every occurrence of its text appears once; `replacements` says how
/// many places it went.
fn preview(planned: &[Planned<'_>]) -> String {
    let mut out = String::new();
    for edit in planned {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str("```diff\n");
        for line in edit.old.lines() {
            out.push('-');
            out.push_str(&quote(line));
            out.push('\n');
        }
        for line in edit.text.lines() {
            out.push('+');
            out.push_str(&quote(line));
            out.push('\n');
        }
        out.push_str("```");
    }
    out
}

/// The most one call will load into memory.
///
/// A rewrite needs the whole file, so this is not a budget on what the model
/// sees — it is a bound on what one model-authored path can cost. A source file
/// is orders of magnitude below it; a file above it is one `bash` can reach.
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Reads the file, refusing the paths that would not end.
///
/// A device node or a fifo answers a read for as long as it is asked, and nothing
/// here would stop the asking: the turn would hang and the string would grow until
/// the machine had none left. [`super::read`] refuses the same paths for the same
/// reason.
async fn read_source(path: &Path) -> Result<String, String> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) if !metadata.is_file() => {
            let message = format!("{} is not a file.", path.display());
            let hint = if metadata.is_dir() {
                "It is a directory. Name a file inside it."
            } else {
                "It exists but is not a regular file — a device, a socket or a pipe \
                 answers a read for as long as it is asked, so there is no telling \
                 where that would stop."
            };
            return Err(failure_text(message, Some(hint)));
        }
        Ok(_) => {}
        Err(err) => return Err(open_failure(path, &err)),
    }

    let file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(err) => return Err(open_failure(path, &err)),
    };
    // One byte past the cap, so a file exactly on it is not mistaken for one
    // over it. The bound is on the read rather than on a length taken beforehand,
    // because the file can grow between the two.
    let mut content = String::new();
    let read = match file.take(MAX_BYTES + 1).read_to_string(&mut content).await {
        Ok(read) => read,
        Err(err) => return Err(open_failure(path, &err)),
    };
    if read as u64 > MAX_BYTES {
        return Err(failure_text(
            format!(
                "{} is larger than the {MAX_BYTES}-byte limit for one edit.",
                path.display()
            ),
            Some(
                "A file this size is not something to rewrite through a string. \
                  `bash` can reach it.",
            ),
        ));
    }
    Ok(content)
}

/// The line ending every line in `content` uses, or `None` when they disagree.
///
/// A model writes `\n` whatever the file holds, so on a file of CRLF lines an
/// exact match would fail for a reason the refusal would blame on indentation.
/// A file with mixed endings has no single ending to convert to, and choosing one
/// would rewrite the model's text into an ending the rest of the file does not
/// use — failing to match for a difference no refusal would ever name.
fn uniform_line_ending(content: &str) -> Option<&'static str> {
    if !content.contains("\r\n") {
        return Some("\n");
    }
    (content.matches("\r\n").count() == content.matches('\n').count()).then_some("\r\n")
}

/// Puts text onto the line ending the file uses, whatever the model wrote.
///
/// Through LF first either way: a model that spelled its replacement `\r\n` into
/// an LF file would otherwise leave the file holding two kinds of line ending.
/// With no single ending to match, the text is left as sent — the file and the
/// call then agree about what was asked for, which is the only thing a refusal
/// can point at.
fn to_line_ending(text: &str, ending: Option<&str>) -> String {
    match ending {
        None => text.to_owned(),
        Some("\n") => text.replace("\r\n", "\n"),
        Some(_) => text.replace("\r\n", "\n").replace('\n', "\r\n"),
    }
}

/// The refusal for two strings that were different until they were converted.
///
/// Checked here as well as in `validate`, because conversion is what makes them
/// equal: `"a\r\n"` and `"a\n"` are two strings to the caller and one to the file,
/// and a rewrite of a line with itself is a replacement that reports itself as one.
fn same_after_conversion(edits: &[Edit]) -> Result<(), String> {
    let same: Vec<usize> = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| edit.old_string == edit.new_string)
        .map(|(index, _)| index)
        .collect();
    if same.is_empty() {
        return Ok(());
    }
    let listed = same
        .iter()
        .map(|index| format!("/edits/{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "{listed}: old_string and new_string are the same once put onto the file's own line \
         endings, so this changes nothing"
    ))
}

/// The 1-indexed line an offset falls on.
fn line_of(content: &str, offset: usize) -> usize {
    content[..offset].matches('\n').count() + 1
}

/// The whole of one line, without its ending.
fn line_text(content: &str, line: usize) -> &str {
    content
        .lines()
        .nth(line.saturating_sub(1))
        .unwrap_or_default()
}

/// The opening of a file, for a refusal whose only other option is a guess.
///
/// What is actually there settles what the refusal cannot: whether the file
/// indents with tabs or spaces, and how deep the text the model wrote sits. A
/// refusal that names neither leaves the model to guess between the two.
///
/// Bounded by bytes as well as lines, because a minified bundle is one line: a
/// line limit alone would put a refusal of megabytes in front of a model whose
/// problem was a single missing space.
fn opening_lines(content: &str) -> String {
    let mut out = String::new();
    for line in content.lines().take(PREVIEW_LINES) {
        if out.len() + line.len() >= PREVIEW_BYTES {
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

/// Text inside a fence, with any fence of its own pushed out of the way so the
/// block cannot be closed early by what it quotes.
fn quote(text: &str) -> String {
    text.replace("```", "``\u{200b}`")
}

/// Why a file could not be read, phrased for someone who has to decide what to
/// try next.
fn open_failure(path: &std::path::Path, err: &std::io::Error) -> String {
    let shown = path.display();
    let message = format!("Cannot read {shown}: {err}");
    let hint = match err.kind() {
        std::io::ErrorKind::NotFound => {
            "Nothing is at that path, so there is no text to replace. `write` creates a file; \
             `edit` changes one that is already there."
        }
        std::io::ErrorKind::PermissionDenied => "The path exists but this process may not read it.",
        std::io::ErrorKind::IsADirectory => "It is a directory. Name a file inside it.",
        // What `read_to_string` answers for bytes that are not text, which for a
        // coding agent means an image, a binary, or a file in another encoding.
        std::io::ErrorKind::InvalidData => {
            "The bytes are not UTF-8 text, so there is no way to say what to replace. The file \
             may be binary, or in another encoding; `bash` is what reaches those."
        }
        _ => "The path could not be read, and the reason above is the only detail available.",
    };
    failure_text(message, Some(hint))
}

/// Why a matched file could not be written back.
fn write_failure(path: &std::path::Path, err: &std::io::Error) -> String {
    let shown = path.display();
    let message = format!("Cannot write {shown}: {err}");
    let hint = match err.kind() {
        std::io::ErrorKind::PermissionDenied => "The file is not writable by this process.",
        std::io::ErrorKind::NotFound => {
            "The file went away between reading it and writing it back. Read it again."
        }
        _ => "The path could not be written, and the reason above is the only detail available.",
    };
    failure_text(message, Some(hint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use crate::tools::FAILURE_MARKER;

    /// A directory for one test to edit in, emptied first so a rerun starts from
    /// the same place.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("srud-edit-tests").join(name);
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
    async fn edit(dir: &Path, arguments: serde_json::Value) -> ToolOutcome {
        EditTool
            .call(&ctx(dir), arguments)
            .await
            .expect("the arguments match the schema")
    }

    /// One replacement in `note.txt`, which is what most tests want.
    fn one(old: &str, new: &str) -> serde_json::Value {
        at("note.txt", old, new)
    }

    /// One replacement at a named path.
    fn at(path: &str, old: &str, new: &str) -> serde_json::Value {
        serde_json::json!({
            "path": path,
            "edits": [{ "old_string": old, "new_string": new }]
        })
    }

    /// Several replacements in one call, against `note.txt`.
    ///
    /// `replace_all` is a `Value` rather than a `bool` so a test can send it the
    /// way a model does — quoted.
    fn several(
        edits: serde_json::Value,
        replace_all: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let mut call = serde_json::json!({ "path": "note.txt", "edits": edits });
        if let Some(replace_all) = replace_all {
            call["replace_all"] = replace_all;
        }
        call
    }

    /// The reply, asserting the tool reported success.
    fn reply(outcome: &ToolOutcome) -> serde_json::Value {
        assert!(!outcome.is_error, "expected success: {}", outcome.output);
        serde_json::from_str(&outcome.output).expect("the reply is JSON")
    }

    /// The failure text, asserting the tool refused.
    fn refusal(outcome: &ToolOutcome) -> String {
        assert!(outcome.is_error, "expected a refusal: {}", outcome.output);
        outcome.output.clone()
    }

    /// The file's contents after a call.
    fn read_back(dir: &Path, file: &str) -> String {
        std::fs::read_to_string(dir.join(file)).expect("the file to still be there")
    }

    #[tokio::test]
    async fn replaces_text_in_a_file() {
        let dir = scratch_with("replace", "note.txt", "one\ntwo\nthree\n");
        let out = reply(&edit(&dir, one("two", "TWO")).await);

        assert_eq!(read_back(&dir, "note.txt"), "one\nTWO\nthree\n");
        assert_eq!(out["replacements"], 1);
        assert_eq!(
            out["path"].as_str(),
            Some(dir.join("note.txt").display().to_string().as_str()),
            "the reply names the file it actually edited"
        );
    }

    #[tokio::test]
    async fn an_empty_replacement_deletes_the_match() {
        let dir = scratch_with("delete", "note.txt", "keep\ndrop\nkeep\n");

        reply(&edit(&dir, one("drop\n", "")).await);

        assert_eq!(read_back(&dir, "note.txt"), "keep\nkeep\n");
    }

    #[tokio::test]
    async fn leaves_the_rest_of_the_file_alone() {
        // The point of the tool: one line changes and the other 199 come back
        // byte for byte. Compared whole, because checking the count and the two
        // ends would pass on a file whose middle was reordered or duplicated.
        let dir = scratch("surrounding");
        let body: String = (1..=200).map(|n| format!("line {n}\n")).collect();
        std::fs::write(dir.join("big.txt"), &body).expect("fixture file");

        reply(&edit(&dir, at("big.txt", "line 100\n", "LINE ONE HUNDRED\n")).await);

        assert_eq!(
            read_back(&dir, "big.txt"),
            body.replace("line 100\n", "LINE ONE HUNDRED\n"),
            "one line changed and nothing else moved"
        );
    }

    #[tokio::test]
    async fn matches_against_the_file_as_it_is_now_not_the_running_result() {
        // Two edits in one call, each whose text the other would have moved if
        // the second matched against the first's output.
        let dir = scratch_with("batch", "note.txt", "alpha\nbeta\ngamma\n");

        let out = reply(
            &edit(
                &dir,
                several(
                    serde_json::json!([
                        { "old_string": "alpha\n", "new_string": "ALPHA\nbeta\n" },
                        { "old_string": "gamma\n", "new_string": "GAMMA\n" }
                    ]),
                    None,
                ),
            )
            .await,
        );

        assert_eq!(out["replacements"], 2);
        assert_eq!(read_back(&dir, "note.txt"), "ALPHA\nbeta\nbeta\nGAMMA\n");
    }

    #[tokio::test]
    async fn changes_every_occurrence_when_asked() {
        let dir = scratch_with("all", "note.txt", "x = 1\ny = 2\nx = 3\n");

        let out = reply(
            &edit(
                &dir,
                several(
                    serde_json::json!([{ "old_string": "x = ", "new_string": "x := " }]),
                    Some(true.into()),
                ),
            )
            .await,
        );

        assert_eq!(out["replacements"], 2, "both occurrences, counted");
        assert_eq!(read_back(&dir, "note.txt"), "x := 1\ny = 2\nx := 3\n");
    }

    #[tokio::test]
    async fn refuses_a_quoted_boolean_that_is_not_one() {
        // `yes` is what a model reaches for when it is unsure, and reading it as
        // true would be the destructive of the two available readings.
        let dir = scratch_with("quoted-nonsense", "note.txt", "x = 1\n");

        let err = EditTool
            .call(
                &ctx(&dir),
                serde_json::json!({
                    "path": "note.txt",
                    "edits": [{ "old_string": "x = ", "new_string": "x := " }],
                    "replace_all": "yes"
                }),
            )
            .await
            .expect_err("`yes` is not a boolean");

        assert!(matches!(err, ToolError::InvalidArguments { .. }), "{err}");
        assert!(
            err.to_string().contains("`yes` is not a boolean"),
            "and names what it got: {err}"
        );
    }

    #[tokio::test]
    async fn reads_a_quoted_boolean() {
        // Models send `"false"` for a boolean often enough that a strict
        // decoder's type error names the shape rather than the mistake.
        let dir = scratch_with("quoted", "note.txt", "x = 1\nx = 2\n");

        let refusal = refusal(
            &edit(
                &dir,
                several(
                    serde_json::json!([{ "old_string": "x = ", "new_string": "x := " }]),
                    Some(serde_json::json!("false")),
                ),
            )
            .await,
        );

        assert!(refusal.contains("Found 2 matches"), "{refusal}");
        assert_eq!(
            read_back(&dir, "note.txt"),
            "x = 1\nx = 2\n",
            "and `false` was read as false, not as truthy"
        );
    }

    #[tokio::test]
    async fn accepts_a_quoted_true() {
        let dir = scratch_with("quoted-true", "note.txt", "x = 1\nx = 2\n");

        reply(
            &edit(
                &dir,
                several(
                    serde_json::json!([{ "old_string": "x = ", "new_string": "x := " }]),
                    Some(serde_json::json!("true")),
                ),
            )
            .await,
        );

        assert_eq!(read_back(&dir, "note.txt"), "x := 1\nx := 2\n");
    }

    #[tokio::test]
    async fn finds_a_match_in_a_file_of_crlf_lines() {
        // The model writes \n whatever the file holds. Without the conversion
        // this could never match, and the refusal would blame indentation.
        let dir = scratch("crlf");
        std::fs::write(dir.join("win.txt"), "one\r\ntwo\r\nthree\r\n").expect("fixture file");

        reply(&edit(&dir, at("win.txt", "two", "TWO")).await);

        assert_eq!(
            read_back(&dir, "win.txt"),
            "one\r\nTWO\r\nthree\r\n",
            "the file keeps its own line ending throughout"
        );
    }

    #[tokio::test]
    async fn writes_the_replacement_on_the_files_line_ending() {
        let dir = scratch("crlf-new");
        std::fs::write(dir.join("win.txt"), "one\r\ntwo\r\n").expect("fixture file");

        reply(&edit(&dir, at("win.txt", "two", "a\nb")).await);

        assert_eq!(read_back(&dir, "win.txt"), "one\r\na\r\nb\r\n");
    }

    #[tokio::test]
    async fn refuses_text_that_is_not_in_the_file() {
        let dir = scratch_with("missing", "note.txt", "one\ntwo\n");
        let refusal = refusal(&edit(&dir, one("three", "THREE")).await);

        assert!(refusal.contains(FAILURE_MARKER), "{refusal}");
        assert!(
            refusal.contains("Nothing in") && refusal.contains("three"),
            "the refusal says what was looked for: {refusal}"
        );
        assert!(
            refusal.contains("one\ntwo"),
            "and quotes the file: {refusal}"
        );
        assert_eq!(read_back(&dir, "note.txt"), "one\ntwo\n", "nothing changed");
    }

    #[tokio::test]
    async fn points_at_the_lines_an_ambiguous_match_sits_on() {
        // The line numbers are the whole point: with them the next call is a
        // read followed by a quote, not a guess.
        let dir = scratch_with("ambiguous", "note.txt", "a\ndup\nb\ndup\nc\ndup\n");
        let refusal = refusal(&edit(&dir, one("dup", "DUP")).await);

        assert!(refusal.contains("Found 3 matches"), "{refusal}");
        assert!(refusal.contains("at line 2"), "{refusal}");
        assert!(refusal.contains("at line 4"), "{refusal}");
        assert!(refusal.contains("...and 1 more"), "{refusal}");
        assert_eq!(
            refusal.matches("at line").count(),
            2,
            "a file with many matches must not bury the reason: {refusal}"
        );
        assert_eq!(read_back(&dir, "note.txt"), "a\ndup\nb\ndup\nc\ndup\n");
    }

    #[tokio::test]
    async fn refuses_two_replacements_that_overlap() {
        let dir = scratch_with("overlap", "note.txt", "alpha\nbeta\n");
        let refusal = refusal(
            &edit(
                &dir,
                several(
                    serde_json::json!([
                        { "old_string": "alpha\nbeta", "new_string": "x" },
                        { "old_string": "beta", "new_string": "y" }
                    ]),
                    None,
                ),
            )
            .await,
        );

        assert!(refusal.contains("overlap"), "{refusal}");
        assert!(refusal.contains("from line 2"), "{refusal}");
        assert!(
            refusal.contains("beta"),
            "and quotes what they share: {refusal}"
        );
        assert_eq!(
            read_back(&dir, "note.txt"),
            "alpha\nbeta\n",
            "nothing changed"
        );
    }

    #[tokio::test]
    async fn refuses_a_call_that_asks_for_the_same_text_twice() {
        // Not two texts meeting but one text sent twice. Called an overlap, the
        // message would send the model off narrowing two edits when the answer is
        // to send one.
        let dir = scratch_with("same-twice", "note.txt", "alpha\nbeta\n");
        let refusal = refusal(
            &edit(
                &dir,
                several(
                    serde_json::json!([
                        { "old_string": "beta", "new_string": "B" },
                        { "old_string": "beta", "new_string": "b" }
                    ]),
                    None,
                ),
            )
            .await,
        );

        assert!(
            refusal.contains("same text to be replaced twice"),
            "{refusal}"
        );
        assert!(refusal.contains("line 2"), "{refusal}");
        assert_eq!(
            read_back(&dir, "note.txt"),
            "alpha\nbeta\n",
            "nothing changed"
        );
    }

    /// The refusal for arguments, which `validate` makes before the file is
    /// touched — the same call [`ToolRegistry::dispatch`] makes.
    fn refuse_arguments(arguments: serde_json::Value) -> String {
        EditTool
            .validate(&arguments)
            .expect_err("the arguments name no change to make")
    }

    #[tokio::test]
    async fn refuses_an_empty_old_string() {
        let err = refuse_arguments(one("", "x"));

        assert!(err.contains("/edits/0/old_string"), "{err}");
        assert!(
            err.contains("write"),
            "and points at the tool that can: {err}"
        );
    }

    #[tokio::test]
    async fn refuses_a_replacement_that_changes_nothing() {
        let err = refuse_arguments(one("one", "one"));

        assert!(err.contains("/edits/0"), "{err}");
        assert!(
            err.contains("are the same"),
            "and says which of the two it is about: {err}"
        );
    }

    #[tokio::test]
    async fn refuses_an_empty_edits_list() {
        // Asked for a round trip and would change nothing, so it is refused
        // before the file is even read.
        let err = refuse_arguments(serde_json::json!({ "path": "note.txt", "edits": [] }));

        assert!(err.contains("/edits"), "{err}");
    }

    #[tokio::test]
    async fn reports_every_unusable_replacement_at_once() {
        let err = refuse_arguments(serde_json::json!({
            "path": "note.txt",
            "edits": [
                { "old_string": "", "new_string": "x" },
                { "old_string": "a", "new_string": "a" }
            ]
        }));

        assert!(err.contains("/edits/0"), "{err}");
        assert!(err.contains("/edits/1"), "{err}");
    }

    #[tokio::test]
    async fn refuses_to_edit_a_file_that_is_not_there() {
        let dir = scratch("no-file");
        let refusal = refusal(&edit(&dir, one("a", "b")).await);

        assert!(refusal.contains("Cannot read"), "{refusal}");
        assert!(
            refusal.contains("write"),
            "and points at the tool that does create files: {refusal}"
        );
    }

    #[tokio::test]
    async fn refuses_to_edit_a_directory() {
        let dir = scratch("directory");
        std::fs::create_dir(dir.join("thing")).expect("fixture directory");

        let refusal = refusal(&edit(&dir, at("thing", "a", "b")).await);

        assert!(refusal.contains("is a directory"), "{refusal}");
        assert!(dir.join("thing").is_dir(), "and it is left alone");
    }

    #[tokio::test]
    async fn reports_the_path_it_touched() {
        let dir = scratch_with("touched", "note.txt", "one\n");
        let out = edit(&dir, one("one", "two")).await;

        assert_eq!(
            out.touched_paths,
            vec![dir.join("note.txt").display().to_string()],
            "the caller can say what the call changed without parsing the reply"
        );
    }

    #[tokio::test]
    async fn puts_a_quoted_line_ending_onto_the_files_own() {
        // A model that sends `\r\n` into an LF file would otherwise leave the
        // file with two kinds of line ending and no mention of it.
        let dir = scratch_with("quoted-ending", "note.txt", "one\ntwo\n");

        reply(&edit(&dir, one("two", "a\r\nb")).await);

        assert_eq!(
            read_back(&dir, "note.txt"),
            "one\na\nb\n",
            "the replacement took the file's line ending, not the model's"
        );
    }

    #[tokio::test]
    async fn counts_adjacent_matches_the_way_it_can_replace_them() {
        // `aa` occurs twice in `aaa` if matches may share a byte. Replacing both
        // is impossible — the second sits inside the text the first removes — so
        // a count that said two would promise a `replace_all` it cannot deliver.
        let dir = scratch_with("adjacent", "note.txt", "aaa\n");

        let out = reply(
            &edit(
                &dir,
                serde_json::json!({
                    "path": "note.txt",
                    "edits": [{ "old_string": "aa", "new_string": "b" }],
                    "replace_all": true
                }),
            )
            .await,
        );

        assert_eq!(out["replacements"], 1, "one replacement, not two");
        assert_eq!(read_back(&dir, "note.txt"), "ba\n");
    }

    #[tokio::test]
    async fn refuses_a_file_whose_bytes_are_not_text() {
        let dir = scratch("binary");
        std::fs::write(dir.join("blob.bin"), [0xff, 0xfe, 0x00, 0x01]).expect("fixture file");

        let refusal = refusal(&edit(&dir, at("blob.bin", "a", "b")).await);

        assert!(refusal.contains("not UTF-8"), "{refusal}");
        assert!(
            refusal.contains("bash"),
            "and names the tool that does reach such a file: {refusal}"
        );
    }

    #[tokio::test]
    async fn finds_a_match_in_a_file_with_mixed_line_endings() {
        // A file is not all one ending or the other in practice, and a tool that
        // assumed it was would rewrite the model's text into an ending the rest
        // of the file does not use, then refuse for a difference it never names.
        let dir = scratch("mixed-ending");
        std::fs::write(dir.join("m.txt"), "one\r\ntwo\nthree\n").expect("fixture file");

        reply(&edit(&dir, at("m.txt", "two", "TWO")).await);

        assert_eq!(
            read_back(&dir, "m.txt"),
            "one\r\nTWO\nthree\n",
            "each line keeps the ending it had"
        );
    }

    #[tokio::test]
    async fn refuses_a_replacement_that_only_differs_by_how_its_ending_is_spelled() {
        // Normalising both sides can turn two different strings into one, so the
        // "this changes nothing" refusal has to come after that, not before.
        let dir = scratch_with("ending-noop", "f.txt", "x\na\ny\n");

        let refusal = refusal(&edit(&dir, at("f.txt", "a\r\n", "a\n")).await);

        assert!(
            refusal.contains("same"),
            "and not a replacement that rewrites a line with itself: {refusal}"
        );
        assert_eq!(read_back(&dir, "f.txt"), "x\na\ny\n");
    }

    #[tokio::test]
    async fn refuses_a_path_that_is_not_a_regular_file() {
        // Reading a device node never ends, and nothing here bounds it.
        let dir = scratch("device");

        let refusal = refusal(&edit(&dir, at("/dev/null", "a", "b")).await);

        assert!(refusal.contains("not a file"), "{refusal}");
    }

    #[tokio::test]
    async fn edits_a_file_whose_last_line_has_no_newline() {
        // Every other fixture ends in a newline, so nothing else covers a file
        // that does not — and a replacement at the very end runs against that.
        let dir = scratch_with("no-final-newline", "note.txt", "one\ntwo");
        reply(&edit(&dir, at("note.txt", "two", "TWO")).await);

        assert_eq!(
            read_back(&dir, "note.txt"),
            "one\nTWO",
            "and no newline is invented for the line that had none"
        );
    }

    #[tokio::test]
    async fn offsets_land_on_character_boundaries_in_multibyte_text() {
        // Every offset here comes from a match, so it is a character boundary; a
        // slice taken at one that is not would panic rather than edit.
        let dir = scratch_with("multibyte", "note.txt", "héllo\nwörld\nαβγ\n");
        let out = reply(
            &edit(
                &dir,
                several(
                    serde_json::json!([
                        { "old_string": "héllo", "new_string": "bonjour" },
                        { "old_string": "αβγ", "new_string": "delta" }
                    ]),
                    None,
                ),
            )
            .await,
        );

        assert_eq!(out["replacements"], 2);
        assert_eq!(read_back(&dir, "note.txt"), "bonjour\nwörld\ndelta\n");
    }

    #[tokio::test]
    async fn names_the_replacement_it_could_not_find_among_several() {
        // With more than one replacement, a refusal that quotes the text without
        // saying which one it was leaves the model to diff it against what it sent.
        let dir = scratch_with("which-edit", "note.txt", "alpha\nbeta\n");
        let refusal = refusal(
            &edit(
                &dir,
                several(
                    serde_json::json!([
                        { "old_string": "alpha", "new_string": "A" },
                        { "old_string": "gamma", "new_string": "G" }
                    ]),
                    None,
                ),
            )
            .await,
        );

        assert!(refusal.contains("/edits/1"), "{refusal}");
        assert!(refusal.contains("gamma"), "{refusal}");
    }

    #[tokio::test]
    async fn a_refusal_about_one_line_of_a_minified_file_stays_small() {
        // A line limit alone would put megabytes of bundle in front of a model
        // whose problem was one missing space.
        let dir = scratch("minified");
        let bundle = format!("var a={};{}", "x".repeat(200_000), "\nsecond line\n");
        std::fs::write(dir.join("bundle.js"), &bundle).expect("fixture file");

        let refusal = refusal(&edit(&dir, at("bundle.js", "nothing here", "x")).await);

        assert!(
            refusal.len() < 8 * PREVIEW_BYTES,
            "the refusal is {} bytes for a file whose first line is 200k",
            refusal.len()
        );
    }

    #[tokio::test]
    async fn the_reply_shows_what_was_replaced_and_what_took_its_place() {
        let dir = scratch_with("reply", "note.txt", "before\nafter\n");
        let out = reply(&edit(&dir, one("before", "after")).await);

        let diff = out["diff"].as_str().expect("diff");
        assert!(diff.contains("-before"), "{diff}");
        assert!(diff.contains("+after"), "{diff}");
    }

    #[test]
    fn a_quoted_value_cannot_close_the_fence_around_it() {
        // The refusal quotes the text it could not find, and the model put that
        // text there; a fence inside it would end the block early and leave the
        // rest of the refusal looking like more of the quote.
        let quoted = quote("before\n```\nafter");
        assert!(
            !quoted.contains("```"),
            "nothing inside the quote can close the block: {quoted}"
        );
    }

    #[tokio::test]
    async fn the_reply_diff_survives_a_fence_in_the_replaced_text() {
        // The same hazard as the refusal's, on the other side: the reply quotes
        // the model back too, and a fence there would end the block early and
        // leave the rest of the reply looking like more diff.
        let dir = scratch_with("reply-fence", "note.txt", "before\n```\nafter\n");
        let out = reply(&edit(&dir, one("```", "```rust")).await);

        let diff = out["diff"].as_str().expect("diff");
        assert_eq!(
            diff.matches("```").count(),
            2,
            "only the block's own opening and closing fence: {diff}"
        );
        assert!(
            diff.ends_with("```"),
            "and the block closes at the end: {diff}"
        );
    }
}
