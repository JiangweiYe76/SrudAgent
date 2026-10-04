//! The `write` tool: a file's whole contents, replaced.

use std::path::Path;

use serde::Deserialize;

use super::{failure_text, invalid_arguments, resolve, Tool, ToolContext, ToolError, ToolOutcome};

/// The name the model calls.
const NAME: &str = "write";

/// What the model is told the tool does.
const DESCRIPTION: &str = "\
Write `content` to `path` as the file's entire contents, replacing whatever was \
there. `path` may be absolute or relative to the working directory, and any \
directories it names are created if they are missing. Nothing is merged, appended \
or patched: a file that was longer than `content` loses the tail, so anything \
worth keeping has to be in `content` too. Prefer `edit` for changing part of a \
file, which quotes only the text it replaces and leaves the rest of the file \
alone; reach for `write` to create a file or replace all of one. The reply is \
JSON: the resolved path, how many bytes were written, and `created`, which is \
false when an existing file was overwritten.";

/// The arguments the model sends.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    /// Absolute, or relative to the working directory.
    path: String,
    /// The file's entire new contents.
    content: String,
}

/// Writes files in the session's working directory.
pub struct WriteTool;

#[async_trait::async_trait]
impl Tool for WriteTool {
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
                    "description": "Absolute, or relative to the working directory. \
                                    Directories it names are created if missing."
                },
                "content": {
                    "type": "string",
                    "description": "The file's entire new contents, replacing what was there."
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    async fn call(
        &self,
        ctx: &ToolContext,
        arguments: serde_json::Value,
    ) -> Result<ToolOutcome, ToolError> {
        let args: Args = serde_json::from_value(arguments)
            .map_err(|err| invalid_arguments(NAME, err.to_string()))?;
        let path = resolve(ctx, &args.path);
        let shown = path.display();

        // One look at the path before anything is created. It settles both what the
        // reply will say about `created` and whether the name is already taken by
        // something a file cannot take the place of.
        let existing = tokio::fs::metadata(&path).await.ok();
        if let Some(metadata) = &existing {
            if metadata.is_dir() {
                let message = format!("{shown} is a directory.");
                let hint = "Name a file inside it. Writing here would have to remove the \
                            directory, and the files in it with it.";
                return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
            }
            // A pipe answers a write for as long as something reads it, so the call
            // would not return and there is no telling where that would stop.
            if !metadata.is_file() {
                let message = format!("{shown} exists but is not a regular file.");
                let hint = "It is a device, a socket or a pipe. Writing to one goes \
                            somewhere other than a file, and does not end on its own.";
                return Ok(ToolOutcome::failure(failure_text(message, Some(hint))));
            }
        }
        // A path that cannot be looked up is a path this call is about to make, so
        // the write that follows reports it if it turns out to be wrong.
        let created = existing.is_none();

        if let Some(parent) = path.parent() {
            if let Err(err) = tokio::fs::create_dir_all(parent).await {
                return Ok(ToolOutcome::failure(write_failure(&path, &err)));
            }
        }
        if let Err(err) = tokio::fs::write(&path, args.content.as_bytes()).await {
            return Ok(ToolOutcome::failure(write_failure(&path, &err)));
        }

        let reply = serde_json::json!({
            "path": path.display().to_string(),
            "bytes_written": args.content.len(),
            "created": created,
        });
        let written = path.display().to_string();
        Ok(ToolOutcome::success(reply.to_string()).with_paths([written]))
    }
}

/// Why a path could not be written, phrased for someone who has to decide what to
/// try next.
///
/// A bare `No such file or directory (os error 2)` leaves open whether the file's
/// own name is wrong, whether a directory above it is missing, and whether
/// something on the path is a file where a directory belongs — three next steps,
/// none of them the one the reader would pick from that sentence alone.
fn write_failure(path: &Path, err: &std::io::Error) -> String {
    let shown = path.display();
    let message = format!("Cannot write {shown}: {err}");
    let hint = match err.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "The path is not writable by this process. Somewhere above it there may \
             be a directory this process cannot write in, or a file it may not \
             replace."
        }
        std::io::ErrorKind::NotFound => {
            "A directory above the file is missing. Names given relative are resolved \
             against the working directory, so a bare file name lands directly inside \
             it."
        }
        // Making a directory fails this way when a name on the path is already a
        // file, which is a fact about the shape of the path rather than about
        // permission, and the error text does not say so.
        std::io::ErrorKind::AlreadyExists => {
            "A name above the file already exists as a file, so no directory can be \
             put there. The file this call names is not the obstacle; check the \
             directories leading to it."
        }
        // The same shape one level deeper: nothing above the file is a directory,
        // so the walk stops there instead of at the name that is in the way.
        std::io::ErrorKind::NotADirectory => {
            "A name above the file is a file rather than a directory, so the path \
             cannot be walked. Check the directories leading to it."
        }
        std::io::ErrorKind::IsADirectory => "It is a directory. Name a file inside it.",
        _ => {
            "The path could not be written, and the reason above is the only detail \
              available."
        }
    };
    failure_text(message, Some(hint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use crate::tools::FAILURE_MARKER;

    /// A directory for one test to write into, emptied first so a rerun starts from
    /// the same place.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("srud-write-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch directory");
        dir
    }

    fn ctx(dir: &Path) -> ToolContext {
        ToolContext {
            cwd: dir.to_path_buf(),
        }
    }

    /// Runs the tool against `dir`, expecting well-formed arguments.
    async fn write(dir: &Path, arguments: serde_json::Value) -> ToolOutcome {
        WriteTool
            .call(&ctx(dir), arguments)
            .await
            .expect("the arguments match the schema")
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

    #[tokio::test]
    async fn writes_a_new_file() {
        let dir = scratch("new");
        let out = reply(
            &write(
                &dir,
                serde_json::json!({ "path": "note.txt", "content": "hello" }),
            )
            .await,
        );

        assert_eq!(out["created"], true, "nothing was there to replace");
        assert_eq!(out["bytes_written"], 5);
        assert_eq!(
            out["path"].as_str(),
            Some(dir.join("note.txt").display().to_string().as_str()),
            "the reply names the file it actually wrote"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("note.txt")).unwrap(),
            "hello"
        );
    }

    #[tokio::test]
    async fn replaces_the_whole_of_an_existing_file() {
        let dir = scratch("replace");
        std::fs::write(dir.join("note.txt"), "a long first version").expect("fixture file");

        let out = reply(
            &write(
                &dir,
                serde_json::json!({ "path": "note.txt", "content": "short" }),
            )
            .await,
        );

        assert_eq!(out["created"], false, "an existing file was overwritten");
        // The point of the tool: nothing is merged, so the old tail is gone.
        assert_eq!(
            std::fs::read_to_string(dir.join("note.txt")).unwrap(),
            "short",
            "the file holds `content` and nothing else"
        );
    }

    #[tokio::test]
    async fn writes_into_directories_it_creates() {
        let dir = scratch("mkdir");
        let nested = Path::new("src/deep/inner.txt");

        let out = reply(
            &write(
                &dir,
                serde_json::json!({ "path": "src/deep/inner.txt", "content": "made" }),
            )
            .await,
        );

        assert_eq!(out["created"], true);
        assert_eq!(
            std::fs::read_to_string(dir.join(nested)).unwrap(),
            "made",
            "the directories above the file were made, not asked for twice"
        );
    }

    #[tokio::test]
    async fn an_absolute_path_is_taken_as_given_bypassing_the_working_directory() {
        let dir = scratch("absolute-bypass");
        let target = dir.join("elsewhere.txt");

        write(
            &dir,
            serde_json::json!({ "path": target.display().to_string(), "content": "here" }),
        )
        .await;

        assert_eq!(std::fs::read_to_string(&target).unwrap(), "here");
    }

    #[tokio::test]
    async fn empty_content_empties_the_file() {
        // Truncating to nothing is a legitimate thing to ask for, so it is not
        // treated as a missing argument.
        let dir = scratch("empty");
        std::fs::write(dir.join("note.txt"), "was something").expect("fixture file");

        let out = reply(
            &write(
                &dir,
                serde_json::json!({ "path": "note.txt", "content": "" }),
            )
            .await,
        );

        assert_eq!(out["bytes_written"], 0);
        assert_eq!(std::fs::read_to_string(dir.join("note.txt")).unwrap(), "");
    }

    #[tokio::test]
    async fn reports_the_path_it_touched() {
        let dir = scratch("touched");
        let out = write(
            &dir,
            serde_json::json!({ "path": "note.txt", "content": "x" }),
        )
        .await;

        assert_eq!(
            out.touched_paths,
            vec![dir.join("note.txt").display().to_string()],
            "the caller can say what the call changed without parsing the reply"
        );
    }

    #[tokio::test]
    async fn refuses_to_write_over_a_directory() {
        let dir = scratch("directory");
        std::fs::create_dir(dir.join("thing")).expect("fixture directory");

        let refusal =
            refusal(&write(&dir, serde_json::json!({ "path": "thing", "content": "x" })).await);

        assert!(refusal.contains(FAILURE_MARKER), "{refusal}");
        assert!(refusal.contains("is a directory"), "{refusal}");
        assert!(
            dir.join("thing").is_dir(),
            "and the directory is left as it was, files and all"
        );
    }

    #[tokio::test]
    async fn reports_a_path_that_cannot_be_written() {
        let dir = scratch("blocked");
        // A file where a directory has to be: the parent cannot be made, and the
        // error the model sees has to say which part of the path is at fault.
        std::fs::write(dir.join("blocker"), "in the way").expect("fixture file");

        let refusal = refusal(
            &write(
                &dir,
                serde_json::json!({ "path": "blocker/inner.txt", "content": "x" }),
            )
            .await,
        );

        assert!(refusal.contains(FAILURE_MARKER), "{refusal}");
        assert!(
            refusal.contains("blocker/inner.txt"),
            "the refusal names the file that could not be written: {refusal}"
        );
        assert!(
            refusal.contains("exists as a file"),
            "and says the shape of the path is why: {refusal}"
        );
    }

    #[tokio::test]
    async fn reports_a_file_standing_where_a_deeper_directory_belongs() {
        // The same shape as above, one level deeper: `blocker` is a file, so the
        // walk stops at `blocker/nope` rather than at the name `blocker` itself,
        // and the two report different errors.
        let dir = scratch("deep-blocked");
        std::fs::write(dir.join("b2"), "in the way").expect("fixture file");

        let refusal = refusal(
            &write(
                &dir,
                serde_json::json!({ "path": "b2/nope/deep.txt", "content": "x" }),
            )
            .await,
        );

        assert!(refusal.contains(FAILURE_MARKER), "{refusal}");
        assert!(
            refusal.contains("is a file rather than a directory"),
            "and says which part of the path is at fault, rather than leaving the \
             errno to explain itself: {refusal}"
        );
    }

    #[tokio::test]
    async fn refuses_a_path_that_is_not_a_regular_file() {
        // A pipe answers a write for as long as something reads it, so the call
        // would not return.
        let dir = scratch("device");

        let refusal = refusal(
            &write(
                &dir,
                serde_json::json!({ "path": "/dev/null", "content": "x" }),
            )
            .await,
        );

        assert!(refusal.contains("not a regular file"), "{refusal}");
    }

    #[tokio::test]
    async fn an_absolute_path_is_taken_as_given_and_says_so() {
        let dir = scratch("absolute");
        let target = dir.join("elsewhere.txt");

        let out = write(
            &dir,
            serde_json::json!({ "path": target.display().to_string(), "content": "here" }),
        )
        .await;

        assert!(!out.is_error, "expected success: {}", out.output);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "here");
    }

    #[tokio::test]
    async fn refuses_arguments_the_schema_forbids() {
        let dir = scratch("arguments");

        let err = WriteTool
            .call(
                &ctx(&dir),
                serde_json::json!({ "path": "note.txt", "content": "x", "mode": "append" }),
            )
            .await
            .expect_err("`mode` is not a parameter");

        assert!(matches!(err, ToolError::InvalidArguments { .. }), "{err}");
        assert!(
            !dir.join("note.txt").exists(),
            "and nothing was written on the way to refusing"
        );
    }

    #[tokio::test]
    async fn missing_content_is_refused_before_anything_is_created() {
        let dir = scratch("missing-content");

        let err = WriteTool
            .call(&ctx(&dir), serde_json::json!({ "path": "made/deep.txt" }))
            .await
            .expect_err("`content` is required");

        assert!(matches!(err, ToolError::InvalidArguments { .. }), "{err}");
        assert!(
            !dir.join("made").exists(),
            "the directories above the file are not made for a call that cannot run"
        );
    }
}
