//! What a model reads back from a search.
//!
//! The reply is JSON, so these tests read it the way the runtime does rather
//! than matching on prose: a field the model cannot see is a field that does not
//! exist, and a line format that shifts is a format the model has to guess at.

use srud_core::tools::{grep::GrepTool, Tool, ToolContext, ToolOutcome, FAILURE_MARKER};

use std::path::{Path, PathBuf};

/// A directory holding one file, emptied first so a rerun starts from the same
/// place.
fn scratch(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join("srud-grep").join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    for (path, contents) in files {
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("fixture directory");
        }
        std::fs::write(target, contents).expect("fixture file");
    }
    dir
}

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        cwd: dir.to_path_buf(),
    }
}

/// Runs a search and hands back the parsed reply, so a test can read a field
/// rather than match the JSON as text.
async fn search(dir: &Path, arguments: serde_json::Value) -> serde_json::Value {
    let ToolOutcome {
        output, is_error, ..
    } = GrepTool
        .call(&ctx(dir), arguments)
        .await
        .expect("the arguments match the schema");
    assert!(!is_error, "the search succeeded, got: {output}");
    serde_json::from_str(&output).expect("the reply is JSON")
}

/// Runs a search that is expected to be refused.
async fn refused(dir: &Path, arguments: serde_json::Value) -> String {
    let outcome = GrepTool
        .call(&ctx(dir), arguments)
        .await
        .expect("the arguments match the schema");
    assert!(outcome.is_error, "the search was refused");
    outcome.output
}

#[tokio::test]
async fn a_match_is_reported_with_its_path_and_line() {
    let dir = scratch(
        "a-match",
        &[
            ("src/main.rs", "fn main() {\n    println!(\"hello\");\n}\n"),
            (
                "src/lib.rs",
                "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
            ),
        ],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "fn main" })).await;

    assert_eq!(reply["num_files"], 1, "one file matched");
    assert_eq!(reply["num_matches"], 1, "one line matched");
    assert_eq!(
        reply["content"], "src/main.rs:1: fn main() {",
        "the line is reported as path:line: text"
    );
    assert!(
        reply.get("truncated").is_none(),
        "an untruncated search does not claim to be truncated"
    );
}

#[tokio::test]
async fn paths_are_relative_to_the_working_directory() {
    let dir = scratch("relative", &[("deep/nested/file.rs", "needle here\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(
        reply["content"], "deep/nested/file.rs:1: needle here",
        "an absolute filesystem path would be noise to a model, and would name \
         a layout the tool was only told to search under"
    );
}

#[tokio::test]
async fn a_named_file_is_searched_on_its_own() {
    let dir = scratch("one-file", &[("a.rs", "shared\n"), ("b.rs", "shared\n")]);

    let reply = search(
        &dir,
        serde_json::json!({ "pattern": "shared", "path": "a.rs" }),
    )
    .await;

    assert_eq!(reply["num_files"], 1, "only the named file was searched");
    assert_eq!(reply["content"], "a.rs:1: shared");
}

#[tokio::test]
async fn include_narrows_the_search_to_matching_files() {
    let dir = scratch(
        "include",
        &[
            ("a.rs", "needle\n"),
            ("b.txt", "needle\n"),
            ("c.md", "needle\n"),
        ],
    );

    let reply = search(
        &dir,
        serde_json::json!({ "pattern": "needle", "include": "*.rs" }),
    )
    .await;

    assert_eq!(reply["num_files"], 1, "only the .rs file matched");
    assert_eq!(reply["content"], "a.rs:1: needle");
}

#[tokio::test]
async fn a_file_matching_nothing_is_not_an_error() {
    // The model has to be able to tell "nothing matched" from "the search
    // failed": the first means look elsewhere, the second means try again.
    let dir = scratch("no-match", &[("a.rs", "something else\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(reply["num_files"], 0);
    assert_eq!(reply["num_matches"], 0);
    assert_eq!(reply["content"], "", "an empty result is still a result");
}

#[tokio::test]
async fn every_match_is_counted_not_only_the_first() {
    let dir = scratch(
        "counts",
        &[("a.rs", "one\ntwo\nthree\n"), ("b.rs", "four\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "^(one|four)$" })).await;

    assert_eq!(reply["num_matches"], 2, "both matches are counted");
    assert_eq!(reply["num_files"], 2, "across two files");
}

#[tokio::test]
async fn the_limit_caps_the_matches_and_says_so() {
    let dir = scratch("limit", &[("a.rs", "hit\nhit\nhit\nhit\nhit\nhit\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 2 })).await;

    assert_eq!(reply["num_matches"], 2, "the limit was applied");
    assert_eq!(
        reply["truncated"], true,
        "the count is a floor, and the model has to know that"
    );
    let note = reply["content"].as_str().expect("text");
    assert!(
        note.contains("Stopped after 2 matches"),
        "the note says what to narrow, got: {note}"
    );
    assert!(
        note.contains("path") && note.contains("pattern"),
        "the note names the arguments that can be narrowed, got: {note}"
    );
}

#[tokio::test]
async fn a_path_outside_the_working_directory_keeps_its_root() {
    // A leading `/` is the only thing telling the model this file is somewhere
    // it was never pointed at. Dropped, `tmp/elsewhere/a.rs` reads as a path
    // under the working directory, which is a different directory entirely.
    let dir = scratch("outside", &[("a.rs", "needle\n")]);
    let elsewhere = scratch("outside-elsewhere", &[("b.rs", "needle\n")]);

    let reply = search(
        &dir,
        serde_json::json!({ "pattern": "needle", "path": elsewhere.to_str() }),
    )
    .await;

    let content = reply["content"].as_str().expect("text");
    assert!(
        content.starts_with(elsewhere.to_str().expect("path")),
        "the absolute path is reported as given, got: {content}"
    );
    assert!(
        !content.starts_with("tmp/"),
        "a relative-looking path outside the working directory would name \
         somewhere else, got: {content}"
    );
}

#[tokio::test]
async fn a_path_under_the_working_directory_is_reported_without_its_root() {
    let dir = scratch("inside", &[("deep/a.rs", "needle\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(
        reply["content"], "deep/a.rs:1: needle",
        "the working directory is already known, so repeating it is noise"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_symlink_is_reported_as_unsearched_rather_than_skipped_silently() {
    // Following a link would read outside the tree the caller pointed at, and
    // could loop. But a reply that says nothing about it reads as a total, and
    // a search must not give that answer by accident.
    let dir = scratch("symlink", &[("real/a.rs", "needle\n")]);
    let outside = std::env::temp_dir().join("srud-grep-outside");
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(&outside).expect("outside");
    std::fs::write(outside.join("b.rs"), "needle\n").expect("outside file");
    std::os::unix::fs::symlink(&outside, dir.join("linked")).expect("symlink");

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(reply["num_matches"], 1, "only the real file was searched");
    assert!(
        !reply["content"].as_str().expect("text").contains("b.rs"),
        "the link's contents are not in the tree, so they are not in the reply"
    );
    assert_eq!(
        reply["partial"], true,
        "something under the path was left unsearched, and the model has to know"
    );

    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn the_same_search_twice_gives_the_same_lines_in_the_same_order() {
    // The limit cuts the walk off wherever it reaches, so which file that is
    // decides what a narrowed retry would see. Left in readdir order that
    // differs between filesystems and between runs.
    let dir = scratch(
        "stable-order",
        &[
            ("m.rs", "hit\n"),
            ("a.rs", "hit\n"),
            ("z.rs", "hit\n"),
            ("b.rs", "hit\n"),
        ],
    );

    let first = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 2 })).await;
    let second = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 2 })).await;

    assert_eq!(
        first["content"], second["content"],
        "two identical searches must not differ between runs"
    );
    assert!(
        reply_lines(&first).starts_with("a.rs:1: hit\nb.rs:1: hit"),
        "and the order is the sorted one, not the order the filesystem listed, \
         got: {}",
        reply_lines(&first)
    );
}

/// The reported lines without the trailing note, so a test can assert on the
/// order of the results rather than on the wording of the advice.
fn reply_lines(reply: &serde_json::Value) -> &str {
    reply["content"]
        .as_str()
        .expect("text")
        .split("\n\n[")
        .next()
        .expect("the lines before any note")
}

#[tokio::test]
async fn a_limit_that_lands_on_a_file_boundary_still_says_it_was_truncated() {
    // Both files match the limit exactly, so whichever the walk reaches first
    // fills it and ends the search at a file boundary, with the other file
    // never searched. A reply without `truncated` would read as a total.
    let dir = scratch(
        "limit-boundary",
        &[("a.rs", "hit\nhit\n"), ("b.rs", "hit\nhit\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 2 })).await;

    assert_eq!(reply["num_matches"], 2, "the limit was applied");
    assert_eq!(reply["num_files"], 1, "only the first file was searched");
    assert_eq!(
        reply["truncated"], true,
        "one whole file was never searched, so the count is a floor"
    );
    let note = reply["content"].as_str().expect("text");
    assert!(
        note.contains("Stopped after 2 matches"),
        "the note says what to narrow, got: {note}"
    );
}

#[tokio::test]
async fn a_limit_that_equals_the_whole_result_does_not_claim_truncation() {
    // Reaching the limit on the last match of the last file left nothing out.
    // Saying `truncated` anyway would send the model looking for matches that
    // do not exist, which is its own kind of wrong answer.
    let dir = scratch("limit-exact", &[("a.rs", "hit\nhit\nhit\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 3 })).await;

    assert_eq!(reply["num_matches"], 3, "every match came back");
    assert_eq!(reply["num_files"], 1);
    assert!(
        reply.get("truncated").is_none(),
        "nothing was left out, so nothing claims otherwise, got: {}",
        reply["content"]
    );
    assert!(
        !reply["content"]
            .as_str()
            .expect("text")
            .contains("Stopped after"),
        "and the note does not tell the model to narrow a search that was complete"
    );
}

#[tokio::test]
async fn a_limit_reached_with_files_still_to_walk_is_reported_as_a_floor() {
    // The walk had not finished when the limit was reached, and answering
    // whether the rest of the tree matches would mean searching it — the work
    // the limit exists to avoid. So the count is a floor. Erring this way costs
    // the model one narrowed retry; erring the other way would have it trust a
    // count that is missing matches.
    let dir = scratch(
        "limit-floor",
        &[
            ("a.rs", "hit\nhit\n"),
            ("b.rs", "nothing here\n"),
            ("c.rs", "nor here\n"),
        ],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 2 })).await;

    assert_eq!(reply["num_matches"], 2, "the limit was applied");
    assert_eq!(
        reply["truncated"], true,
        "files were left unvisited, so the count is a floor"
    );
}

#[tokio::test]
async fn a_path_that_is_not_there_says_where_the_search_ran() {
    let dir = scratch("missing", &[("a.rs", "needle\n")]);

    let text = refused(
        &dir,
        serde_json::json!({ "pattern": "needle", "path": "nowhere" }),
    )
    .await;

    assert!(
        text.starts_with(FAILURE_MARKER),
        "marked as a failure: {text}"
    );
    assert!(
        text.contains("nowhere") && text.contains("Path does not exist"),
        "the message names the path that was asked for, got: {text}"
    );
    assert!(
        text.contains(&dir.display().to_string()),
        "the message says where the search would have run, since a relative \
         path is the commonest way to be wrong about one, got: {text}"
    );
}

#[tokio::test]
async fn a_pattern_the_engine_cannot_compile_explains_the_syntax() {
    let dir = scratch("bad-pattern", &[("a.rs", "needle\n")]);

    let text = refused(&dir, serde_json::json!({ "pattern": "(?P<n>a" })).await;

    assert!(
        text.contains("not a valid regular expression"),
        "got: {text}"
    );
    assert!(
        text.contains("look-around"),
        "the message names what Rust's regex does not have, rather than \
         leaving the model to guess, got: {text}"
    );
}

#[tokio::test]
async fn a_unc_path_is_refused_without_touching_it() {
    // Statting a UNC path makes the process authenticate against the remote
    // share and hand over its credentials, so the refusal has to come first.
    let dir = scratch("unc", &[("a.rs", "needle\n")]);

    let text = refused(
        &dir,
        serde_json::json!({ "pattern": "needle", "path": r"\\server\share" }),
    )
    .await;

    assert!(text.contains("UNC"), "the reason is named, got: {text}");
    assert!(
        !text.contains("does not exist"),
        "the path was never looked up, so its absence is not the answer, got: {text}"
    );
}

#[tokio::test]
async fn a_version_control_directory_is_not_searched() {
    // A `.git` directory holds object names rather than source, so a match
    // inside one is noise the model has to read past.
    let dir = scratch(
        "vcs",
        &[("a.rs", "needle\n"), (".git/objects/ab/cdef", "needle\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(reply["num_files"], 1, "only the source file matched");
    assert!(!reply["content"].as_str().unwrap().contains(".git"));
}

#[tokio::test]
async fn a_carriage_return_ending_a_line_is_not_reported_as_part_of_the_text() {
    // A file written on Windows ends every line with `\r\n`. The `\r` is a
    // terminator, not text, so reporting it would put a control character in
    // front of every line the model reads.
    let dir = scratch("crlf", &[("a.txt", "alpha\r\nbeta\r\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "alpha" })).await;

    assert_eq!(
        reply["content"], "a.txt:1: alpha",
        "the `\\r` belongs to the line ending, not to the line's text"
    );
}

#[tokio::test]
async fn an_anchored_pattern_does_not_reach_across_a_crlf() {
    // A known limit rather than a choice, pinned so that changing the line
    // terminator shows up here instead of surprising a model mid-task. The
    // searcher is given `\n` alone, which is what lets `foo$` work on a file
    // with Unix endings; the cost is that on a file with Windows endings the
    // `\r` sits between the text and the `\n`, and `$` stops before it.
    //
    // Turning on CRLF mode instead reverses which case breaks, and one searcher
    // cannot know per file which endings that file has. This is also how
    // ripgrep behaves, so a model that has learned `rg` is not misled.
    let dir = scratch(
        "crlf-anchor",
        &[("win.txt", "alpha\r\nbeta\r\n"), ("unix.txt", "alpha\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "alpha$" })).await;

    assert_eq!(
        reply["num_matches"], 1,
        "only the file with Unix endings matches, got: {}",
        reply["content"]
    );
    assert_eq!(
        reply["content"], "unix.txt:1: alpha",
        "so the limitation is a missed match on a CRLF file, not a wrong line"
    );
}

#[tokio::test]
async fn a_long_line_is_shortened_rather_than_crowding_out_the_rest() {
    // A minified or generated file holds lines long enough to fill the reply on
    // their own, which is what the reported width is there to prevent.
    let long: String = std::iter::repeat_n('x', 2_000).collect();
    let dir = scratch(
        "long-line",
        &[("min.js", &format!("needle{long}\n")), ("b.rs", "needle\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    let content = reply["content"].as_str().expect("text");
    for line in content.lines() {
        // Counted the way the width is defined, which is characters and not the
        // bytes `len` reports — a CJK line is three bytes per character and
        // would fail an assertion written in bytes while being within it.
        assert!(
            line.chars().count() <= 520,
            "every line stays near the reported width, got {} chars",
            line.chars().count()
        );
    }
    assert!(
        content.contains('…'),
        "a shortened line is marked, so the model knows it is not the whole line"
    );
    assert!(
        content.contains("b.rs:1: needle"),
        "the file after the long one is still reported, got: {content}"
    );
}

#[tokio::test]
async fn a_multibyte_line_is_measured_in_characters() {
    // The width stands in for tokens, and one CJK character costs about as much
    // as a short word. A cap written in bytes would hold a CJK line to a third
    // of the characters an ASCII one gets, which is not what it is there for.
    let long: String = std::iter::repeat_n('中', 600).collect();
    let dir = scratch("multibyte", &[("a.txt", &format!("needle{long}\n"))]);

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    let content = reply["content"].as_str().expect("text");
    let body = content.split_once(": ").expect("path and body").1;
    assert!(
        body.chars().count() <= 501,
        "600 characters are cut to the reported 500, got {}",
        body.chars().count()
    );
    assert!(content.contains('…'), "and the cut is marked");
}

#[tokio::test]
async fn a_binary_file_does_not_end_the_search() {
    // One binary file must not hide every match after it, which is what quitting
    // on the first NUL byte would do.
    let dir = scratch(
        "binary",
        &[("a.bin", "needle\0\0\0binary\n"), ("b.rs", "needle\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(
        reply["num_matches"], 2,
        "the match after the binary file is still reported, got: {}",
        reply["content"]
    );
}

#[tokio::test]
async fn case_insensitivity_is_available_to_the_pattern() {
    // Rather than an argument, `(?i)` is part of the regex, so it costs no
    // parameter and composes with everything else the pattern can say.
    let dir = scratch("case", &[("a.rs", "Needle here\n")]);

    let reply = search(&dir, serde_json::json!({ "pattern": "(?i)needle" })).await;

    assert_eq!(reply["num_matches"], 1, "the inline flag took effect");
}

#[tokio::test]
async fn a_gitignore_is_honoured() {
    // Build output is the bulk of most trees and never the answer, so it is
    // excluded without the caller having to remember to.
    let dir = scratch("ignored", &[]);
    std::fs::create_dir_all(dir.join("target")).expect("build directory");
    std::fs::write(dir.join("target/generated.rs"), "needle\n").expect("generated file");
    std::fs::write(dir.join("a.rs"), "needle\n").expect("source file");
    std::fs::write(dir.join(".gitignore"), "target/\n").expect("ignore file");

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(
        reply["num_files"], 1,
        "only the source file matched, so the ignore file was read, got: {}",
        reply["content"]
    );
}

#[test]
fn arguments_are_checked_before_anything_is_searched() {
    // A refusal costs no round trip, and reporting every problem at once means
    // the model can fix them in one call rather than one per turn.
    let problems = GrepTool
        .validate(&serde_json::json!({
            "pattern": "a\nb",
            "include": "!*.rs,*.ts",
            "limit": -1,
        }))
        .expect_err("these three are all wrong");

    assert!(problems.contains("/pattern"), "got: {problems}");
    assert!(problems.contains("/include"), "got: {problems}");
    assert!(problems.contains("/limit"), "got: {problems}");
}

#[test]
fn an_empty_pattern_or_include_is_refused() {
    // Both are one step away from a search that does the wrong thing silently.
    // An empty pattern matches every line, which floods the reply and answers
    // nothing; an empty glob is read as no filter at all, so the search would
    // quietly cover files the caller meant to exclude.
    for arguments in [
        serde_json::json!({ "pattern": "" }),
        serde_json::json!({ "pattern": "x", "include": "" }),
        serde_json::json!({ "pattern": "x", "include": "   " }),
    ] {
        assert!(
            GrepTool.validate(&arguments).is_err(),
            "refused: {arguments}"
        );
    }
}

#[test]
fn a_comma_inside_braces_is_an_alternation_not_a_list() {
    // `*.{rs,ts}` is one glob naming two kinds of file, where a bare `a,b` is
    // two patterns and would need an argument each.
    assert!(
        GrepTool
            .validate(&serde_json::json!({ "pattern": "x", "include": "*.{rs,ts}" }))
            .is_ok(),
        "a brace alternation is one glob"
    );
    assert!(
        GrepTool
            .validate(&serde_json::json!({ "pattern": "x", "include": "*.rs,*.ts" }))
            .is_err(),
        "a bare comma-separated list is not"
    );
}

#[test]
fn the_schema_asks_for_an_object_and_names_the_pattern() {
    let tool = GrepTool;
    let parameters = tool.parameters();

    assert_eq!(parameters["type"], "object");
    assert_eq!(parameters["required"], serde_json::json!(["pattern"]));
    assert_eq!(
        parameters["additionalProperties"], false,
        "an argument the tool does not read is one the model should not send"
    );
    assert_eq!(
        tool.name(),
        "grep",
        "the name is what the model calls, so it is part of the contract"
    );
}

#[tokio::test]
async fn a_search_under_a_gitignore_excluding_everything_finds_nothing() {
    // A tree that ignores everything is unusual but not a failure; the reply
    // says so in the same shape as any other empty result.
    let dir = scratch("all-ignored", &[]);
    std::fs::write(dir.join("a.rs"), "needle\n").expect("source file");
    std::fs::write(dir.join(".gitignore"), "*\n").expect("ignore file");

    let reply = search(&dir, serde_json::json!({ "pattern": "needle" })).await;

    assert_eq!(reply["num_matches"], 0, "got: {}", reply["content"]);
}

#[tokio::test]
async fn a_limit_of_zero_asks_for_no_limit_and_says_nothing_was_left_out() {
    // `limit: 0` is the one value that removes the brake, so it has to mean
    // "no limit" rather than "no matches" — and a search that stopped short
    // would have to say so, which it does not here.
    let dir = scratch(
        "unlimited",
        &[("a.rs", "hit\nhit\nhit\n"), ("b.rs", "hit\n")],
    );

    let reply = search(&dir, serde_json::json!({ "pattern": "hit", "limit": 0 })).await;

    assert_eq!(reply["num_matches"], 4, "every match came back");
    assert_eq!(reply["num_files"], 2);
    assert!(
        reply.get("truncated").is_none(),
        "nothing was left out, so nothing claims otherwise"
    );
}
