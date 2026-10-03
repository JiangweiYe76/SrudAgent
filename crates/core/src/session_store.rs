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
use crate::types::{ResponseItem, TurnEndReason, TurnId};

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
}
