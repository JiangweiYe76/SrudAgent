//! The `grep` tool: search file contents by regular expression.
//!
//! The engine is the one ripgrep itself uses, as a library. Spawning an `rg`
//! binary instead would leave the tool unusable on a machine that has not
//! installed one, and would make its behaviour depend on whichever version
//! that machine carries.

use std::path::Path;

use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use serde::Deserialize;
use tokio::task::spawn_blocking;

use super::{failure_text, invalid_arguments, resolve, Tool, ToolContext, ToolError, ToolOutcome};

/// The name the model calls.
const NAME: &str = "grep";

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Search file contents by regular expression. `path` may be absolute or relative \
to the working directory and may name a file or a directory; it defaults to the \
working directory. `include` narrows the search to files matching one glob, \
such as `*.rs`. Returns up to 250 matches, each on its own line as \
`path:line: text` with the 1-indexed line number. The reply is JSON: the \
`content`, how many files and matches it covers, and `truncated` when matches \
were left out. Use this rather than running grep or rg through `bash` — the \
search is bounded and its paths are reported relative to the working directory. \
A line is returned whole or shortened at 500 characters, marked with `…`; to \
read a match in full, read that file at the line given.";

/// How many matches one call returns, so a broad pattern cannot flood the
/// context. A caller that needs more can narrow the path or the pattern.
const DEFAULT_LIMIT: usize = 250;

/// The most one matched line contributes. A minified or generated file can hold
/// a line long enough to crowd out everything else, and this is the width past
/// which the line is reported shortened rather than whole. Counted in
/// characters rather than bytes: it stands in for tokens, and one CJK character
/// costs about as much as a short word rather than the quarter of a token an
/// ASCII letter does.
const MAX_LINE_CHARS: usize = 500;

/// Directories skipped whatever the pattern is. A `.git` directory holds object
/// names and packed blobs rather than source, so a search that entered one
/// would report noise the model then has to read past.
const SKIPPED_DIRECTORIES: &[&str] = &[".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// The regular expression to look for. Line-oriented: a pattern cannot span
    /// lines.
    pattern: String,
    /// Absolute, or relative to the working directory.
    path: Option<String>,
    /// One glob selecting which files to search.
    include: Option<String>,
    /// How many matches to return at most. Defaults to 250; 0 means no limit,
    /// which floods the context on a broad pattern.
    limit: Option<usize>,
}

/// Searches file contents for a regular expression.
pub struct GrepTool;

#[async_trait::async_trait]
impl Tool for GrepTool {
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
                "pattern": {
                    "type": "string",
                    "description": "The regular expression to search for. Matches a whole line at a time; use (?i) to ignore case."
                },
                "path": {
                    "type": "string",
                    "description": "File or directory to search. Defaults to the working directory."
                },
                "include": {
                    "type": "string",
                    "description": "One glob naming which files to search, e.g. \"*.rs\". Not a list; a leading ! is not accepted."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 0,
                    "description": "The most matches to return. Defaults to 250. Pass 0 for no limit."
                }
            },
            "required": ["pattern"],
            "additionalProperties": false
        })
    }

    fn validate(&self, arguments: &serde_json::Value) -> Result<(), String> {
        // Every problem is reported at once, naming each by its path, so the
        // model can fix them in one call rather than one per turn.
        let mut problems: Vec<String> = Vec::new();
        // Only the rules a JSON schema cannot state are checked here. A value
        // of the wrong type is already answered by the deserialization in `call`,
        // and saying so twice would report a problem the caller has not got.
        if let Some(pattern) = arguments.get("pattern").and_then(serde_json::Value::as_str) {
            if pattern.is_empty() {
                problems.push("/pattern: must not be empty".to_owned());
            } else if pattern.contains('\n') {
                problems.push(
                    "/pattern: must not contain a line break, since a match is one whole line"
                        .to_owned(),
                );
            }
        }
        if let Some(glob) = arguments.get("include").and_then(serde_json::Value::as_str) {
            problems.extend(include_problems(glob));
        }
        if arguments.get("limit").is_some()
            && arguments
                .get("limit")
                .and_then(serde_json::Value::as_u64)
                .is_none()
        {
            problems.push("/limit: must be a whole number of matches".to_owned());
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

        let limit = args.limit.unwrap_or(DEFAULT_LIMIT);
        // A UNC path is refused before it is resolved, since resolving one
        // anchors it to the working directory and the check would then read as
        // a path that merely does not exist. On Windows, statting a UNC path
        // makes the process authenticate against the remote share and hand over
        // its credentials.
        if args.path.as_deref().is_some_and(is_unc) {
            let shown = args.path.as_deref().unwrap_or_default().to_owned();
            let message = format!("{shown} is a UNC network path.");
            let hint = "Search a path on this machine instead, or a directory this \
                        session was started in.";
            return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
        }

        let target = match &args.path {
            Some(path) => resolve(ctx, path),
            None => ctx.cwd.clone(),
        };

        let include = args.include.as_deref().map(str::to_owned);
        let pattern = args.pattern;
        let cwd = ctx.cwd.clone();

        // The search is a blocking walk over the filesystem, so it runs off the
        // runtime's threads. A caller that gives up waiting for it does not stop
        // it — the walk finishes and its result is dropped.
        let search =
            spawn_blocking(move || search(&cwd, &target, &pattern, include.as_deref(), limit))
                .await
                .map_err(|err| ToolError::InvalidArguments {
                    name: NAME.to_owned(),
                    message: format!("the search task did not finish: {err}"),
                })?;

        match search {
            Ok(found) => Ok(ToolOutcome::success(found.into_json().to_string())),
            Err(SearchError::NoSuchPath { shown, cwd }) => {
                let message = format!("Path does not exist: {shown}.");
                let hint = format!("The working directory is {cwd}.");
                Ok(ToolOutcome::failure(failure_text(message, Some(&hint))))
            }
            Err(SearchError::BadPattern(detail)) => {
                let message = format!("`pattern` is not a valid regular expression: {detail}.");
                let hint = "Rust's regex syntax has no backreferences or look-around. \
                            For a literal brace, escape it as `\\{`; to ignore case, \
                            prefix the pattern with `(?i)`.";
                Ok(ToolOutcome::failure(failure_text(message, Some(hint))))
            }
            Err(SearchError::Include(detail)) => {
                let message = format!("`include` is not a usable glob: {detail}.");
                let hint = "Give one glob, such as `*.rs` or `crates/**\\*.rs`.";
                Ok(ToolOutcome::failure(failure_text(message, Some(hint))))
            }
        }
    }
}

/// What a completed search reports back.
struct Found {
    /// The matched lines, already formatted.
    content: String,
    /// How many files contributed a line.
    num_files: usize,
    /// How many lines matched.
    num_matches: usize,
    /// Whether the count above is a floor rather than a total.
    truncated: bool,
    /// Whether something under the path was left unsearched — a file that could
    /// not be read, or a symlink that was not followed.
    partial: bool,
}

impl Found {
    fn into_json(self) -> serde_json::Value {
        let mut reply = serde_json::json!({
            "content": self.content,
            "num_files": self.num_files,
            "num_matches": self.num_matches,
        });
        // `truncated` says the count above is a floor rather than a total, and
        // `partial` says the search did not cover everything it was pointed at.
        // Either is the one thing a caller cannot infer from the lines alone.
        if self.truncated {
            reply["truncated"] = serde_json::Value::Bool(true);
        }
        if self.partial {
            reply["partial"] = serde_json::Value::Bool(true);
        }
        reply
    }
}

/// Why a search produced nothing.
enum SearchError {
    /// The path to search is not there.
    NoSuchPath { shown: String, cwd: String },
    /// The pattern did not compile.
    BadPattern(String),
    /// The `include` glob could not be turned into a filter.
    Include(String),
}

/// Runs the search. Called on a blocking thread, not on the runtime.
fn search(
    cwd: &Path,
    target: &Path,
    pattern: &str,
    include: Option<&str>,
    limit: usize,
) -> Result<Found, SearchError> {
    if !target.exists() {
        return Err(SearchError::NoSuchPath {
            shown: target.display().to_string(),
            cwd: cwd.display().to_string(),
        });
    }

    // The line terminator is `\n` alone rather than `\r\n`, which is what keeps
    // an anchored pattern working on a file with Unix endings. CRLF mode would
    // make `foo$` match on a file written on Windows, but it also stops `foo$`
    // matching on a file that has Unix endings, since the pattern would then be
    // looking for a terminator the file does not use. One searcher cannot know
    // which endings each file has, so the more common case wins and the
    // `text_of` below strips a trailing `\r` from what is reported.
    let matcher = RegexMatcher::new_line_matcher(pattern)
        .map_err(|err| SearchError::BadPattern(err.to_string()))?;

    let mut walker = WalkBuilder::new(target);
    // Hidden files are searched because a dotfile is as likely to hold the
    // answer as anything else, and the ignore files below decide what is noise.
    walker
        .hidden(false)
        .parents(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .require_git(false)
        // Windows resolves `Src/A.rs` and `src/a.rs` to one file, so a search
        // that reported both would count one file twice.
        .ignore_case_insensitive(cfg!(windows))
        // A link out of the tree would take the search somewhere the caller
        // never pointed it, so links are not followed.
        .follow_links(false)
        // Entries come back in readdir order otherwise, which differs between
        // filesystems and between runs. Sorted, the same search twice gives the
        // same lines in the same order, and the limit cuts the tree at a place
        // a caller can predict rather than wherever the filesystem happened to
        // stop.
        .sort_by_file_name(|a, b| a.cmp(b));

    if let Some(glob) = include {
        let mut overrides = OverrideBuilder::new(target);
        // A `!` here would subtract files, which is a second filter wearing one
        // argument's clothes; `validate` has already refused it.
        overrides
            .add(glob)
            .map_err(|err| SearchError::Include(err.to_string()))?;
        let built = overrides
            .build()
            .map_err(|err| SearchError::Include(err.to_string()))?;
        walker.overrides(built);
    }

    let mut searcher = SearcherBuilder::new()
        .line_number(true)
        // A file with a NUL in it is not text to answer a question from, and
        // `convert` reports the line rather than ending the search, so one
        // binary file does not hide every match after it.
        .binary_detection(grep_searcher::BinaryDetection::convert(0))
        .build();

    let mut collected = Collector::new(limit);
    let mut entries = walker.build().peekable();
    while let Some(entry) = entries.next() {
        let entry = match entry {
            Ok(entry) => entry,
            // A file we cannot open is skipped: the model is better served by
            // the matches beside it than by an error naming it.
            Err(_) => {
                collected.incomplete = true;
                continue;
            }
        };
        match entry.file_type() {
            Some(kind) if kind.is_file() => {}
            // A symlink is reported rather than followed. Following one would
            // read outside the tree the caller pointed at, and could loop; but
            // saying nothing would let a reply that skipped it read as a total,
            // which is the one answer a search must not give by accident.
            Some(kind) if kind.is_symlink() => collected.incomplete = true,
            _ => {}
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_skipped(path) {
            continue;
        }
        let shown = relative_to(path, cwd);
        collected.begin_file();
        if searcher
            .search_path(&matcher, path, &mut collected.with_file(&shown))
            .is_err()
        {
            // A file that cannot be read is left out rather than failing the
            // search: the matches beside it still answer the question. The
            // search is reported as partial, since a file the caller named may
            // be the one that was skipped.
            collected.incomplete = true;
        }
        if collected.full() {
            // The limit was reached on this file's last line, so `push_match`
            // never saw the extra match that would have marked the truncation
            // itself. Whether anything was actually left out depends on files
            // the walk has not reached, and answering that would mean searching
            // them — which is the work the limit exists to avoid. So this is a
            // floor rather than a count: pending entries make the number a
            // floor, and being wrong that way sends the model to look once more
            // for something that is not there, where being wrong the other way
            // would have it trust an answer that is missing matches.
            if entries.peek().is_some() {
                collected.truncated = true;
            }
            break;
        }
    }

    let mut content = collected.finish();
    if collected.truncated {
        // The count is the reason a caller might look for the rest, so the note
        // says where to narrow rather than only that there is more. A limit of
        // zero asked for no limit, so nothing was left out and this cannot be
        // reached.
        content.push_str(&format!(
            "\n\n[Stopped after {limit} matches. Narrow `path` or `include`, or make \
             the pattern more specific.]"
        ));
    }

    Ok(Found {
        content,
        num_files: collected.files,
        num_matches: collected.matches,
        truncated: collected.truncated,
        partial: collected.incomplete,
    })
}

/// Accumulates matches across files and stops once the limit is reached.
struct Collector {
    /// How many matches to keep. Zero means no limit, which the caller is
    /// warned about in the description.
    limit: usize,
    content: String,
    matches: usize,
    files: usize,
    /// Whether a file being searched now has been counted into `files`.
    counted_this_file: bool,
    /// Whether a match was turned away, or the walk was cut short with files
    /// still to visit.
    truncated: bool,
    /// Whether something under the path was left unsearched.
    incomplete: bool,
}

impl Collector {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            content: String::new(),
            matches: 0,
            files: 0,
            counted_this_file: false,
            truncated: false,
            incomplete: false,
        }
    }

    /// A view of this collector that labels its lines with `path`, for the
    /// duration of one file's search.
    fn with_file<'a>(&'a mut self, path: &'a str) -> Labelled<'a> {
        Labelled { inner: self, path }
    }

    /// Marks a new file as uncounted, so a file is counted once however many of
    /// its lines matched.
    fn begin_file(&mut self) {
        self.counted_this_file = false;
    }

    /// Whether the limit has been reached and no more may be collected.
    fn full(&self) -> bool {
        self.limit != 0 && self.matches >= self.limit
    }

    /// Records one matched line, reporting whether to keep going.
    fn push_match(&mut self, path: &str, line: u64, text: &str) -> bool {
        if self.full() {
            self.truncated = true;
            return false;
        }
        let rendered = format!("{}:{}: {}\n", path, line, shorten(text));
        self.content.push_str(&rendered);
        self.matches += 1;
        true
    }

    /// Counts a file that has contributed at least one match.
    fn count_file(&mut self) {
        if !self.counted_this_file {
            self.files += 1;
            self.counted_this_file = true;
        }
    }

    fn finish(&self) -> String {
        self.content.trim_end().to_owned()
    }
}

/// One file's worth of searching, which knows the path its lines belong to.
struct Labelled<'a> {
    inner: &'a mut Collector,
    path: &'a str,
}

impl Sink for Labelled<'_> {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        self.inner.count_file();
        let text = text_of(mat.bytes());
        Ok(self
            .inner
            .push_match(self.path, mat.line_number().unwrap_or(0), &text))
    }
}

/// The line as text, without its terminator.
///
/// A line that is not UTF-8 comes back lossy rather than failing the search, so
/// a binary file still shows the match that led into it.
fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\n', '\r'])
        .to_owned()
}

/// A line cut to `MAX_LINE_CHARS`, marked so the model knows it is shortened.
fn shorten(line: &str) -> String {
    let mut chars = line.chars();
    let kept: String = chars.by_ref().take(MAX_LINE_CHARS).collect();
    if chars.next().is_some() {
        format!("{kept}…")
    } else {
        kept
    }
}

/// Whether any part of a path is one the search does not enter.
///
/// Every component is checked rather than only the directories walked, so a
/// path that reaches inside one by naming it is refused the same way. Naming a
/// file called `.git` is not something a caller means, and letting it through
/// would make the rule depend on how the path was spelled.
fn is_skipped(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| SKIPPED_DIRECTORIES.contains(&name))
    })
}

/// Whether a path is written as a UNC network path, in either spelling.
///
/// Spelled on the text rather than on a resolved path, since resolving one
/// anchors it to the working directory and the two leading separators stop
/// being visible as the start of anything.
fn is_unc(path: &str) -> bool {
    // `\\server\share` is the native spelling; `//server/share` is what a model
    // writes, and Windows treats it as the same thing.
    path.starts_with(r"\\") || path.starts_with("//")
}

/// `path` as a `/`-separated string relative to `cwd`, falling back to the path
/// itself when it lies outside the working directory.
fn relative_to(path: &Path, cwd: &Path) -> String {
    // Only the leading `/` or `C:\` is dropped when the path really is under
    // the working directory, since the model already knows where that is. A
    // path outside it keeps its root: `tmp/elsewhere/a.rs` names somewhere the
    // model was never told about, and a leading `/` is what says so.
    let Ok(under) = path.strip_prefix(cwd) else {
        return path.to_string_lossy().replace('\\', "/");
    };
    let mut parts: Vec<String> = Vec::new();
    for part in under.components() {
        // A Windows prefix is implied by the working directory, and the root is
        // too, so neither is repeated on what is already relative.
        if matches!(
            part,
            std::path::Component::Prefix(_) | std::path::Component::RootDir
        ) {
            continue;
        }
        parts.push(part.as_os_str().to_string_lossy().into_owned());
    }
    parts.join("/")
}

/// What is wrong with an `include` value, if anything.
///
/// Only one positive glob is accepted. A list would need an argument per
/// pattern, and a `!` would subtract files, which is a second filter wearing
/// this argument's clothes. A comma is a separator only outside braces, because
/// inside them it is the alternation of `*.{rs,ts}`.
fn include_problems(glob: &str) -> Vec<String> {
    let mut problems = Vec::new();
    if glob.trim().is_empty() {
        problems.push("/include: must not be empty when given".to_owned());
        return problems;
    }
    if glob.starts_with('!') {
        problems.push(
            "/include: must be a positive glob; a leading ! would exclude files and is not supported"
                .to_owned(),
        );
        return problems;
    }
    let mut depth = 0usize;
    for character in glob.chars() {
        match character {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                problems.push(
                    "/include: must be one glob, not a comma-separated list; use {a,b} for alternation"
                        .to_owned(),
                );
                return problems;
            }
            _ => {}
        }
    }
    problems
}
