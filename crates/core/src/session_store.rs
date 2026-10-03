//! Writing a session's log: the append-only record behind every session.
//!
//! # The ordering rule
//!
//! A record is written and flushed **before** the event announcing it goes out.
//! The other order loses data: a crash in between leaves the consumer showing
//! something the log does not have, and rebuilding from the log drops it.
//!
//! That rule is not left to the caller. [`SessionLogWriter::record`] performs the
//! write, the flush, and only then returns so the caller may emit — and the turn
//! loop reaches the sink through exactly one call. Six write sites each remembering
//! the order would be six chances to get it backwards, and the mistake is
//! invisible until a crash, which no test reproduces by accident.
//!
//! # What is not here
//!
//! No index, no lookup, no recovery. This writes a log and reads it back; what a
//! consumer builds from that is its own business.

use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::session_event::{Logged, SessionEvent};
use crate::types::{ResponseItem, SessionId, TurnEndReason, TurnId};

/// Where one session's log lives, and what to write in it.
///
/// Holds the open file rather than a path it reopens per record: the log is
/// written once per history entry, and reopening to append would put a syscall
/// on a path the turn loop takes between a model call and the next one.
///
/// One writer at a time is the only requirement — the log is append-only and a
/// turn runs one step at a time. `&self` throughout rather than `&mut self` so a
/// turn loop can hold one by reference for its whole run, the way it already
/// holds a sink, instead of threading a mutable borrow through five functions.
///
/// The lock is tokio's, not std's, and the reason is not a preference: a write is
/// a syscall, so the guard is held across an `.await`, and a `std::sync::MutexGuard`
/// is not `Send`. Holding one would make every future that touched this type
/// unsendable and `tokio::spawn` would reject the turn outright.
#[derive(Debug)]
pub struct SessionLogWriter {
    file: tokio::sync::Mutex<tokio::fs::File>,
    path: PathBuf,
}

impl SessionLogWriter {
    /// Opens a session's log, creating it and its directories if needed.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or the file cannot be opened. A
    /// session that cannot record is a session that cannot be recovered, so this
    /// is reported rather than papered over — the caller decides whether to
    /// refuse the session or to run without a log.
    pub async fn create(path: impl Into<PathBuf>) -> Result<Self, io::Error> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await?;

        Ok(Self {
            file: tokio::sync::Mutex::new(file),
            path,
        })
    }

    /// The file being written.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes one record and flushes it.
    ///
    /// The line is stamped here, with the moment of the write: a reader places a
    /// record by when it reached the log.
    ///
    /// Returns only once the record is on disk. A caller that emits after this
    /// returns is therefore emitting something already durable.
    ///
    /// # Errors
    ///
    /// Fails if the record cannot be written or flushed. The error is returned
    /// rather than swallowed: a log that silently stopped accepting records
    /// would produce a session that looks fine and cannot be recovered.
    pub async fn record(&self, entry: &SessionEvent) -> Result<(), io::Error> {
        let mut line =
            serde_json::to_string(&Logged::now(entry.clone())).map_err(io::Error::other)?;
        line.push('\n');

        // Serialising before taking the lock: nothing here needs the file, and
        // the lock is held across two awaits below.
        let mut file = self.file.lock().await;
        file.write_all(line.as_bytes()).await?;
        file.flush().await
    }

    /// Records a turn's end.
    ///
    /// # Errors
    ///
    /// As [`SessionLogWriter::record`].
    pub async fn end_turn(&self, turn_id: TurnId, reason: TurnEndReason) -> Result<(), io::Error> {
        self.record(&SessionEvent::TurnEnded { turn_id, reason })
            .await
    }

    /// Records one history entry, already stamped with its turn and message.
    ///
    /// # Errors
    ///
    /// As [`SessionLogWriter::record`].
    pub async fn record_item(
        &self,
        turn_id: TurnId,
        message_id: Option<crate::session_event::MessageId>,
        item: &ResponseItem,
    ) -> Result<(), io::Error> {
        self.record(&SessionEvent::item(turn_id, message_id, item.clone()))
            .await
    }
}

/// One record read back, with the line it came from.
///
/// The line number is what makes a skipped record reportable: a log that has
/// lost a record is worth pointing at.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadEntry {
    /// One-based, counting every line in the file.
    pub line: usize,
    /// When the line was written.
    pub time: DateTime<Utc>,
    /// What happened.
    pub event: SessionEvent,
}

/// A line that could not be read, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedLine {
    /// One-based, as [`ReadEntry::line`].
    pub line: usize,
    /// The text that did not parse.
    pub text: String,
    /// What was wrong with it.
    pub reason: SkipReason,
}

/// Why a line was not read as a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The line is not JSON, or not a record this build knows.
    ///
    /// Skipped rather than fatal because the format is versioned and a newer
    /// build may have written a variant this one does not have. A log with such
    /// a line is still a log with every other line readable.
    Unrecognised,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unrecognised => f.write_str("not a record this build knows"),
        }
    }
}

/// What reading a log produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadLog {
    /// The records that were read, in order.
    pub entries: Vec<ReadEntry>,
    /// The lines that were not, in order.
    pub skipped: Vec<SkippedLine>,
}

/// A session rebuilt from its log.
///
/// A log is a sequence of events; a session needs the history the model saw, and
/// a client being shown the session needs the conversation it was part of. Those
/// are not the same list — the env-context block is in one and not the other — so
/// both are built here rather than left to each caller to filter, and the repairs
/// that make them usable are decided once.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    /// The session the log names.
    pub session_id: SessionId,
    /// The directory the session works in.
    pub cwd: PathBuf,
    /// The format version the log was written at.
    ///
    /// Carried rather than checked: a newer log is read for what this build
    /// understands and the rest skipped, so the session is partially recovered
    /// instead of refused.
    pub version: u32,
    /// The title, as last set.
    pub title: Option<String>,
    /// The history the model should see next, oldest first.
    pub history: Vec<ResponseItem>,
    /// What a client replaying the session should be shown, oldest first.
    pub conversation: Vec<SessionEvent>,
    /// Why each turn ended, oldest first.
    ///
    /// A client being shown a session has no `session/prompt` response to close
    /// its turns, so the reasons are read from here: a turn left without an end
    /// record is in [`repairs`], and its reason is the one written there.
    pub turn_ends: Vec<(TurnId, TurnEndReason)>,
    /// Records to append before the session runs again, closing what the log left
    /// open.
    pub repairs: Vec<SessionEvent>,
}

/// Why a log could not be rebuilt into a session.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The log could not be read.
    #[error("cannot read the session log: {0}")]
    Io(#[from] io::Error),

    /// The log holds no session record, so it names no session.
    #[error("the session log has no session record")]
    NoSession,
}

/// Reads a session's log and rebuilds the session it describes.
///
/// For a session that stopped. What its log is missing was lost, so it is
/// repaired; a session that is still open wants [`load_running`], where what is
/// missing has simply not happened yet.
///
/// # Errors
///
/// Fails if the log cannot be read, or if it holds no session record — without
/// one there is no id and no directory to work in.
pub async fn load(path: impl AsRef<Path>) -> Result<Loaded, LoadError> {
    rebuild(&read(path).await?, true)
}

/// What a log says about the session it holds.
///
/// Enough to list a session without opening it: [`load`] also reads every entry
/// to rebuild a history and to close what the log left open, and a client
/// listing every session pays that once per session.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    /// The session the log belongs to.
    pub session_id: SessionId,
    /// The directory the session works in.
    pub cwd: PathBuf,
    /// The last title the log recorded, if it recorded one.
    pub title: Option<String>,
    /// When the log was last written to, which is when the session last did
    /// anything.
    pub updated_at: DateTime<Utc>,
}

/// Reads just enough of a session's log to describe it.
///
/// # Errors
///
/// As [`load`]. A log that cannot be read describes no session, and a caller
/// listing every log skips it rather than failing over one.
pub async fn summarize(path: impl AsRef<Path>) -> Result<Summary, LoadError> {
    summarize_log(&read(path).await?)
}

fn summarize_log(log: &ReadLog) -> Result<Summary, LoadError> {
    let mut title = None;
    let mut updated_at = None;
    let mut session = None;
    for entry in &log.entries {
        match &entry.event {
            SessionEvent::Session {
                session_id, cwd, ..
            } => {
                session = Some((*session_id, cwd.clone()));
            }
            SessionEvent::Title { title: last } => title = last.clone(),
            // Everything else is the session doing its work, which a description
            // of the session has no use for.
            SessionEvent::System { .. }
            | SessionEvent::TurnStarted { .. }
            | SessionEvent::Item { .. }
            | SessionEvent::TurnEnded { .. } => {}
        }
        updated_at = Some(entry.time);
    }

    let (session_id, cwd) = session.ok_or(LoadError::NoSession)?;
    // Unreachable alongside the line above, which a log can only pass by holding a
    // record, and that record is an entry the loop took the time of. Reported as
    // the same absence rather than unwrapped, so a log that somehow got here is a
    // log describing no session rather than a panic.
    let updated_at = updated_at.ok_or(LoadError::NoSession)?;
    Ok(Summary {
        session_id,
        cwd,
        title,
        updated_at,
    })
}

/// Rebuilds a session that has not stopped.
///
/// Nothing is repaired: a turn with no end is one still running, and a call with
/// no result is one still out. This is what a client loading a session another
/// client already has open is shown.
///
/// # Errors
///
/// As [`load`].
pub async fn load_running(path: impl AsRef<Path>) -> Result<Loaded, LoadError> {
    rebuild(&read(path).await?, false)
}

/// Rebuilds the session a log describes.
///
/// `stopped` says whether a log that stops mid-turn is read as a session that
/// stopped, and so needs its gaps repaired, or as one that is still going.
fn rebuild(log: &ReadLog, stopped: bool) -> Result<Loaded, LoadError> {
    let Some((session_id, cwd, version)) =
        log.entries.iter().find_map(|entry| match &entry.event {
            SessionEvent::Session {
                session_id,
                cwd,
                version,
            } => Some((*session_id, cwd.clone(), *version)),
            _ => None,
        })
    else {
        return Err(LoadError::NoSession);
    };

    let mut title = None;
    // Turns the log opened and never closed. A turn's end record can fail to
    // write while the turn itself still ends, so more than one can be open.
    let mut open: Vec<TurnId> = Vec::new();
    // Every recorded entry, in order.
    let mut items: Vec<&SessionEvent> = Vec::new();
    let mut turn_ends: Vec<(TurnId, TurnEndReason)> = Vec::new();

    for entry in &log.entries {
        match &entry.event {
            SessionEvent::Title { title: last } => title = last.clone(),
            SessionEvent::TurnStarted { turn_id } => open.push(*turn_id),
            SessionEvent::TurnEnded { turn_id, reason } => {
                // A turn ends once. A log carrying a second end for one is
                // damaged, and the first is the one that says what happened, so
                // a client is never shown two reasons for the same turn.
                if open.contains(turn_id) {
                    open.retain(|id| id != turn_id);
                    turn_ends.push((*turn_id, *reason));
                }
            }
            SessionEvent::Item { .. } => items.push(&entry.event),
            SessionEvent::Session { .. } | SessionEvent::System { .. } => {}
        }
    }

    // A session that has not stopped has nothing to repair: what the log is
    // missing is what has not happened yet.
    let repairs = if stopped {
        repairs(&items, &open)
    } else {
        Vec::new()
    };

    // A turn that only the repairs closed still ended, and a client replaying it
    // is told so: otherwise the last turn of an interrupted session would have no
    // reason and would look like it were still running.
    turn_ends.extend(repairs.iter().filter_map(|event| match event {
        SessionEvent::TurnEnded { turn_id, reason } => Some((*turn_id, *reason)),
        _ => None,
    }));

    // The repairs that carry an item are part of both the history and the
    // conversation; the one that only closes a turn is in neither, since a turn
    // boundary is not something the model or the client is shown.
    let ordered = items.iter().copied().chain(
        repairs
            .iter()
            .filter(|event| event.history_item().is_some()),
    );

    let history = ordered
        .clone()
        .filter_map(|event| event.history_item().cloned())
        .collect();
    let conversation = ordered
        .filter(|event| {
            event
                .history_item()
                .is_none_or(|item| !crate::context::is_item(item))
        })
        .cloned()
        .collect();

    Ok(Loaded {
        session_id,
        cwd,
        version,
        title,
        history,
        conversation,
        turn_ends,
        repairs,
    })
}

/// The records that close what the log left open.
///
/// A log stops where the process did, so what is missing is what was never
/// written: a turn with no end, which a reader cannot tell from a turn still
/// running, and a tool call with no result, which most model APIs refuse. A call
/// that was answered is left alone, since a result is recorded immediately after
/// its call and anything later belongs to a turn that got that far.
fn repairs(items: &[&SessionEvent], open: &[TurnId]) -> Vec<SessionEvent> {
    let answered: std::collections::HashSet<&str> = items
        .iter()
        .filter_map(|event| match event.history_item() {
            Some(ResponseItem::FunctionCallOutput { call_id, .. }) => Some(call_id.as_str()),
            _ => None,
        })
        .collect();

    let mut repairs: Vec<SessionEvent> = items
        .iter()
        .filter_map(|event| match event.history_item() {
            Some(ResponseItem::FunctionCall { call_id, name, .. })
                if !answered.contains(call_id.as_str()) =>
            {
                Some(SessionEvent::item(
                    // The call's turn, which the call itself was recorded inside.
                    event.turn_id().expect("a call is recorded inside a turn"),
                    None,
                    ResponseItem::FunctionCallOutput {
                        call_id: call_id.clone(),
                        output: unanswered_text(name),
                        is_error: true,
                    },
                ))
            }
            _ => None,
        })
        .collect();

    // Last, so the log ends with the turns closed rather than with a record
    // inside one.
    repairs.extend(open.iter().map(|turn_id| SessionEvent::TurnEnded {
        turn_id: *turn_id,
        reason: TurnEndReason::Interrupted,
    }));
    repairs
}

/// What the model is told about a call whose result never arrived.
///
/// Different from a tool that ran and failed: nothing ran, and saying which is
/// what keeps the model from retrying something that was already attempted.
fn unanswered_text(name: &str) -> String {
    crate::tools::failure_text(
        format!("The `{name}` call did not finish. The session stopped before it returned."),
        Some("The tool may or may not have run. Check before calling it again."),
    )
}

/// Reads a log back.
///
/// # Errors
///
/// Fails only if the file cannot be opened. A line that does not parse is
/// reported in [`ReadLog::skipped`] and does not fail the read: one unreadable
/// line must not cost the reader every other line, which is the difference
/// between a partially-read session and no session at all.
pub async fn read(path: impl AsRef<Path>) -> Result<ReadLog, io::Error> {
    let file = tokio::fs::File::open(path).await?;
    let mut lines = BufReader::new(file).lines();

    let mut log = ReadLog::default();
    let mut number = 0;
    while let Some(line) = lines.next_line().await? {
        number += 1;
        // A trailing newline leaves one empty line, which is not a record.
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Logged>(&line) {
            Ok(logged) => log.entries.push(ReadEntry {
                line: number,
                time: logged.time,
                event: logged.event,
            }),
            Err(_) => log.skipped.push(SkippedLine {
                line: number,
                text: line,
                reason: SkipReason::Unrecognised,
            }),
        }
    }

    Ok(log)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_event::MessageId;
    use crate::types::SessionId;

    /// A path under the system temp directory, unique to this test.
    fn temp_log(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "srud-session-log-{}-{name}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// An empty log at a fresh path, and the path to read it back from.
    async fn fresh(name: &str) -> (SessionLogWriter, PathBuf) {
        let path = temp_log(name);
        let writer = SessionLogWriter::create(&path)
            .await
            .expect("a log to open");
        (writer, path)
    }

    /// A turn id for a test to record against.
    fn turn() -> TurnId {
        TurnId::new()
    }

    #[tokio::test]
    async fn writes_one_json_object_per_line() {
        let (writer, path) = fresh("lines").await;
        let turn_id = turn();

        writer
            .record(&SessionEvent::session(
                SessionId::new(),
                PathBuf::from("/w"),
            ))
            .await
            .expect("the first record");
        writer
            .record_item(turn_id, None, &ResponseItem::assistant("hi"))
            .await
            .expect("a record");
        writer
            .end_turn(turn_id, TurnEndReason::Completed)
            .await
            .expect("the end");

        let text = std::fs::read_to_string(&path).expect("the log on disk");
        let lines: Vec<_> = text.lines().collect();

        assert_eq!(lines.len(), 3, "one line per record: {text}");
        for line in &lines {
            serde_json::from_str::<Logged>(line)
                .unwrap_or_else(|e| panic!("not a record: {line}: {e}"));
        }
        assert!(text.ends_with('\n'), "every record ends its line");
    }

    #[tokio::test]
    async fn reads_back_what_was_written() {
        let (writer, path) = fresh("roundtrip").await;
        let turn_id = turn();
        let message_id = MessageId::new();

        writer
            .record(&SessionEvent::session(
                SessionId::new(),
                PathBuf::from("/w"),
            ))
            .await
            .expect("the first record");
        writer
            .record_item(
                turn_id,
                Some(message_id.clone()),
                &ResponseItem::assistant("hi"),
            )
            .await
            .expect("a record");
        writer
            .end_turn(turn_id, TurnEndReason::Completed)
            .await
            .expect("the end");

        let log = read(&path).await.expect("a log to read");

        assert!(log.skipped.is_empty(), "nothing skipped: {:?}", log.skipped);
        assert_eq!(log.entries.len(), 3);
        assert_eq!(log.entries[1].line, 2, "lines are numbered from one");
        let item = log.entries[1].event.history_item().unwrap();
        assert_eq!(item, &ResponseItem::assistant("hi"));
    }

    #[tokio::test]
    async fn the_time_is_kept_to_the_millisecond() {
        // Truncated as it is taken, so the instant a reader gets back is the one
        // written. Compared against a captured `Logged::now`, this would race the
        // clock the writer reads.
        let (writer, path) = fresh("time-roundtrip").await;

        writer
            .record(&SessionEvent::system("# Role"))
            .await
            .expect("a record");

        let log = read(&path).await.expect("a log to read");
        let time = log.entries[0].time;
        assert_eq!(
            time.timestamp_subsec_nanos() % 1_000_000,
            0,
            "nothing below a millisecond survives: {time:?}"
        );
    }

    #[tokio::test]
    async fn a_record_is_on_disk_before_the_write_returns() {
        // The ordering rule rests on this: whatever the caller does next, the
        // record is already readable by a fresh reader.
        let (writer, path) = fresh("durable").await;
        let turn_id = turn();

        writer
            .end_turn(turn_id, TurnEndReason::Completed)
            .await
            .expect("the end");

        // Read through a separate handle — not the writer — so this sees what
        // is on disk rather than what the writer still holds.
        let log = read(&path).await.expect("a log to read");
        assert_eq!(log.entries.len(), 1);
        assert!(matches!(
            log.entries[0].event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Completed,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn an_unreadable_line_does_not_cost_the_others() {
        // The format is versioned. A newer build may write a record this one has
        // never heard of, and losing the whole session over it would make the
        // format un-extendable.
        let path = temp_log("skip");
        {
            let writer = SessionLogWriter::create(&path)
                .await
                .expect("a log to open");
            writer
                .end_turn(turn(), TurnEndReason::Completed)
                .await
                .expect("before");
        }

        // Appended rather than written over: the point is that a line this build
        // cannot read sits *between* records it can.
        let mut text = std::fs::read_to_string(&path).unwrap();
        // A line this build understands the envelope of, wrapping an event type it
        // has never heard of: the format is versioned, so this is what a newer
        // build's record looks like, not corruption.
        text.push_str(
            "{\"time\":\"2026-10-03T21:28:14.123Z\",\"event\":{\"type\":\"from_the_future\",\"payload\":{\"x\":1}}}\n",
        );
        text.push_str(&format!(
            "{{\"time\":\"2026-10-03T21:28:14.124Z\",\"event\":{{\"type\":\"turn_ended\",\"turn_id\":\"{}\",\"reason\":\"interrupted\"}}}}\n",
            turn()
        ));
        std::fs::write(&path, text).unwrap();

        let log = read(&path).await.expect("a log to read");

        assert_eq!(log.entries.len(), 2, "the two known records survive");
        assert_eq!(log.skipped.len(), 1);
        assert_eq!(log.skipped[0].line, 2, "reported by position");
        assert_eq!(log.skipped[0].reason, SkipReason::Unrecognised);
        // The record after the unreadable line is still read, so one bad line
        // costs that line and not the rest of the session.
        assert!(matches!(
            log.entries[1].event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Interrupted,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_truncated_line_does_not_cost_the_others() {
        // A process that died mid-write leaves a partial last line. It is the
        // common case of the one above, and it must not read as corruption.
        let (writer, path) = fresh("truncated").await;
        writer
            .record(&SessionEvent::session(
                SessionId::new(),
                PathBuf::from("/w"),
            ))
            .await
            .expect("the first record");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"type\":\"item\",\"turn_id\":\"not-fini");
        std::fs::write(&path, text).unwrap();

        let log = read(&path).await.expect("a log to read");

        assert_eq!(log.entries.len(), 1, "the complete record survives");
        assert_eq!(log.skipped.len(), 1);
        assert!(
            log.skipped[0].text.ends_with("not-fini"),
            "the raw text is kept"
        );
    }

    #[tokio::test]
    async fn appends_rather_than_replacing() {
        // A second writer on the same log must extend it. Overwriting would
        // lose the session it was asked to record.
        let path = temp_log("append");
        let first = SessionLogWriter::create(&path)
            .await
            .expect("a log to open");
        first
            .end_turn(turn(), TurnEndReason::Completed)
            .await
            .expect("the first end");

        let second = SessionLogWriter::create(&path)
            .await
            .expect("the same log again");
        second
            .end_turn(turn(), TurnEndReason::Interrupted)
            .await
            .expect("the second end");

        let log = read(&path).await.expect("a log to read");
        assert_eq!(log.entries.len(), 2, "both records are present");
        assert!(matches!(
            log.entries[1].event,
            SessionEvent::TurnEnded {
                reason: TurnEndReason::Interrupted,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn the_trailing_newline_is_not_a_record() {
        let (writer, path) = fresh("trailing").await;
        writer
            .end_turn(turn(), TurnEndReason::Completed)
            .await
            .expect("the end");

        let log = read(&path).await.expect("a log to read");

        assert_eq!(log.entries.len(), 1, "one record, no phantom second line");
        assert!(
            log.skipped.is_empty(),
            "the empty line is not a skipped one: {:?}",
            log.skipped
        );
    }

    #[tokio::test]
    async fn creates_the_directories_it_needs() {
        // The log lives under a dated directory, so the caller should not have to
        // build that path itself.
        let path = temp_log("nested")
            .join("2026")
            .join("10")
            .join("03")
            .join("session.jsonl");
        let writer = SessionLogWriter::create(&path)
            .await
            .expect("a log to open");
        writer
            .end_turn(turn(), TurnEndReason::Completed)
            .await
            .expect("the end");

        assert_eq!(writer.path(), path.as_path());
        assert!(path.is_file(), "the log is where it was asked for");
        if let Some(root) = path.ancestors().nth(3) {
            let _ = std::fs::remove_dir_all(root);
        }
    }

    /// A log built from events, as a reader would have found them.
    fn log_of(events: Vec<SessionEvent>) -> ReadLog {
        ReadLog {
            entries: events
                .into_iter()
                .enumerate()
                .map(|(index, event)| ReadEntry {
                    line: index + 1,
                    time: chrono::Utc::now(),
                    event,
                })
                .collect(),
            skipped: Vec::new(),
        }
    }

    /// A session record naming a session that works in `/w`.
    fn opening() -> SessionEvent {
        SessionEvent::session(SessionId::new(), PathBuf::from("/w"))
    }

    /// A user message, as the loop records one.
    fn said(turn_id: TurnId, text: &str) -> SessionEvent {
        SessionEvent::item(turn_id, None, ResponseItem::user(text))
    }

    #[test]
    fn a_finished_turn_rebuilds_without_repairs() {
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            said(turn_id, "hello"),
            SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Completed,
            },
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(loaded.cwd, PathBuf::from("/w"));
        assert_eq!(loaded.version, crate::session_event::FORMAT_VERSION);
        assert_eq!(loaded.history, vec![ResponseItem::user("hello")]);
        assert!(loaded.repairs.is_empty(), "{:?}", loaded.repairs);
    }

    #[test]
    fn a_log_that_stopped_mid_turn_is_closed_as_interrupted() {
        // Without the synthetic end, a reader cannot tell a turn that was
        // interrupted from one still running.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            said(turn_id, "hello"),
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(
            loaded.repairs,
            vec![SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Interrupted,
            }]
        );
    }

    #[test]
    fn every_turn_left_open_is_closed() {
        // A turn's end record can fail to write while the turn itself still ends,
        // so a log can carry more than one turn that never closed.
        let first = TurnId::new();
        let second = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id: first },
            said(first, "one"),
            SessionEvent::TurnStarted { turn_id: second },
            said(second, "two"),
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(
            loaded.repairs,
            vec![
                SessionEvent::TurnEnded {
                    turn_id: first,
                    reason: TurnEndReason::Interrupted,
                },
                SessionEvent::TurnEnded {
                    turn_id: second,
                    reason: TurnEndReason::Interrupted,
                },
            ]
        );
    }

    #[test]
    fn a_call_with_no_result_is_answered() {
        // A model API refuses a call nothing answered, so a resumed session could
        // not send a request at all without this.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            SessionEvent::item(
                turn_id,
                None,
                ResponseItem::FunctionCall {
                    call_id: "call_1".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
            ),
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        let repaired = loaded
            .repairs
            .iter()
            .find_map(|event| event.history_item())
            .expect("an answer was supplied");
        match repaired {
            ResponseItem::FunctionCallOutput {
                call_id, is_error, ..
            } => {
                assert_eq!(call_id, "call_1");
                assert!(is_error, "nothing ran, so the answer says so");
            }
            other => panic!("expected a tool result: {other:?}"),
        }
        // And the answer is part of the history the model will read.
        assert!(
            loaded.history.iter().any(|item| matches!(
                item,
                ResponseItem::FunctionCallOutput { call_id, .. } if call_id == "call_1"
            )),
            "{:?}",
            loaded.history
        );
    }

    #[test]
    fn an_answered_call_is_left_alone() {
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            SessionEvent::item(
                turn_id,
                None,
                ResponseItem::FunctionCall {
                    call_id: "call_1".into(),
                    name: "read".into(),
                    arguments: "{}".into(),
                },
            ),
            SessionEvent::item(
                turn_id,
                None,
                ResponseItem::FunctionCallOutput {
                    call_id: "call_1".into(),
                    output: "contents".into(),
                    is_error: false,
                },
            ),
            SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Completed,
            },
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert!(loaded.repairs.is_empty(), "{:?}", loaded.repairs);
        assert_eq!(loaded.history.len(), 2);
    }

    #[test]
    fn the_conversation_leaves_out_what_the_user_never_said() {
        // The env-context block is recorded so the model reads it; a client
        // replaying the session is not being shown the agent's own notes.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            SessionEvent::item(
                turn_id,
                None,
                crate::context::item(std::path::Path::new("/w")),
            ),
            said(turn_id, "hello"),
            SessionEvent::item(
                turn_id,
                Some(MessageId::new()),
                ResponseItem::Reasoning {
                    content: "thinking".into(),
                },
            ),
            SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Completed,
            },
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        // The model sees the context; the client does not.
        assert_eq!(loaded.history.len(), 3, "{:?}", loaded.history);
        let shown: Vec<&ResponseItem> = loaded
            .conversation
            .iter()
            .filter_map(SessionEvent::history_item)
            .collect();
        assert_eq!(
            shown,
            vec![
                &ResponseItem::user("hello"),
                // Reasoning is model-invisible but the client renders it, so the
                // two projections are not each other's complement.
                &ResponseItem::Reasoning {
                    content: "thinking".into()
                },
            ],
            "the env-context block is the one left out"
        );
    }

    #[test]
    fn the_last_title_wins() {
        let log = log_of(vec![
            opening(),
            SessionEvent::title(Some("first".into())),
            SessionEvent::title(Some("second".into())),
            SessionEvent::title(None),
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(loaded.title, None, "a cleared title clears it");
    }

    #[test]
    fn a_running_session_is_not_repaired() {
        // Loading a session another client already has open: the turn with no end
        // is one still running, not one that stopped, so nothing is invented for
        // it.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            said(turn_id, "hello"),
        ]);

        let loaded = rebuild(&log, false).expect("a session");

        assert!(loaded.repairs.is_empty(), "{:?}", loaded.repairs);
        assert_eq!(loaded.history.len(), 1, "and nothing was added to it");
    }

    #[test]
    fn a_log_with_no_session_record_names_no_session() {
        let log = log_of(vec![SessionEvent::system("# Role")]);

        assert!(matches!(rebuild(&log, true), Err(LoadError::NoSession)));
    }

    #[test]
    fn every_turn_says_how_it_ended() {
        // A client being shown the session has no prompt response to close its
        // turns with, so each reason has to be readable from here.
        let first = TurnId::new();
        let second = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id: first },
            said(first, "one"),
            SessionEvent::TurnEnded {
                turn_id: first,
                reason: TurnEndReason::Completed,
            },
            SessionEvent::TurnStarted { turn_id: second },
            said(second, "two"),
            SessionEvent::TurnEnded {
                turn_id: second,
                reason: TurnEndReason::Interrupted,
            },
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(
            loaded.turn_ends,
            vec![
                (first, TurnEndReason::Completed),
                (second, TurnEndReason::Interrupted),
            ],
            "in the order the turns ran"
        );
    }

    #[test]
    fn a_turn_only_the_repairs_closed_still_says_it_ended() {
        // Without this the last turn of a session that stopped mid-turn would be
        // shown with no reason at all, and would look like it were still running.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            said(turn_id, "hello"),
        ]);

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(
            loaded.turn_ends,
            vec![(turn_id, TurnEndReason::Interrupted)],
            "the reason the repair gives it"
        );
    }

    #[test]
    fn a_running_turn_says_nothing_about_how_it_will_end() {
        // Its end has not happened yet, so inventing one would report a turn as
        // over that is not.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            said(turn_id, "hello"),
        ]);

        let loaded = rebuild(&log, false).expect("a session");

        assert!(loaded.turn_ends.is_empty(), "{:?}", loaded.turn_ends);
    }

    #[test]
    fn a_turn_that_ended_twice_is_counted_once() {
        // Nothing writes a second end, so a log carrying one is damaged. The first
        // is kept, so a client is shown one reason per turn however many the log
        // claims.
        let turn_id = TurnId::new();
        let log = log_of(vec![
            opening(),
            SessionEvent::TurnStarted { turn_id },
            said(turn_id, "hello"),
            SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Completed,
            },
        ]);
        let mut log = log;
        log.entries.push(ReadEntry {
            line: 4,
            time: chrono::Utc::now(),
            event: SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Interrupted,
            },
        });

        let loaded = rebuild(&log, true).expect("a session");

        assert_eq!(loaded.turn_ends, vec![(turn_id, TurnEndReason::Completed)]);
    }
}
