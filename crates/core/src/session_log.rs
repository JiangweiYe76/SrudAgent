//! The seam between the turn loop and a session's durable log.
//!
//! # Why a trait and not a `SessionLogWriter`
//!
//! The turn loop needs *something* to write to, and in production that is a file.
//! In a test it usually is not: most of what the loop does — ordering of events,
//! interrupting, calling tools twice — has nothing to do with durability, and
//! making every such test build a temporary log would put the filesystem in the
//! way of tests that have no reason to touch it.
//!
//! So the loop takes this trait, and [`Volatile`] is the implementation that keeps
//! records in memory. It is named for what it is rather than hidden behind an
//! `Option`, because "this turn is not recorded" is a decision someone makes
//! deliberately — a test, or a host that has decided durability is not worth the
//! cost — and not one that should be easy to arrive at by omission.
//!
//! # The ordering rule
//!
//! Whoever implements this must make the record readable before returning. The
//! loop relies on it: it announces a record only after this call has resolved, so
//! a consumer never sees something the log does not have. An implementation that
//! buffers and returns early breaks that, and breaks it invisibly.

use std::sync::Mutex;

use async_trait::async_trait;

use crate::session_event::{Logged, MessageId, SessionEvent};
use crate::types::{ResponseItem, TurnEndReason, TurnId};

/// Why a record could not be written.
///
/// Reported rather than swallowed: a log that quietly stopped accepting records
/// produces a session that looks fine in every respect the user can see and cannot
/// be recovered.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    /// The underlying store refused the record.
    #[error("cannot record {what}: {source}")]
    Io {
        /// What was being recorded, for the message.
        what: String,
        /// What the store said.
        #[source]
        source: std::io::Error,
    },
}

/// Where a turn's records go.
///
/// One method, called once per recorded thing. Keeping it to one is what lets the
/// loop treat "written" as a single fact: there is no second path that writes
/// without saying so.
#[async_trait]
pub trait SessionLog: Send + Sync {
    /// Makes one record readable, or says why it could not be.
    ///
    /// The time is stamped here, not carried in by the caller: every line has one
    /// and none of them names the moment it was taken, so an implementation that
    /// forgot would produce a line a reader could not place.
    ///
    /// Returning means durable. The loop announces a record only after this
    /// resolves, so a caller that has seen an event is looking at something the
    /// log already holds.
    async fn record(&self, entry: &SessionEvent) -> Result<(), RecordError>;
}

#[async_trait]
impl SessionLog for crate::session_store::SessionLogWriter {
    async fn record(&self, entry: &SessionEvent) -> Result<(), RecordError> {
        self.record(entry).await.map_err(|source| RecordError::Io {
            what: describe(entry),
            source,
        })
    }
}

/// A log that keeps records in memory and forgets them.
///
/// For a caller that has decided not to persist. Recording still *happens*, so the
/// loop runs the same path either way and only the durability differs — which is
/// what makes it a stand-in rather than a bypass.
#[derive(Debug, Default)]
pub struct Volatile {
    records: Mutex<Vec<Logged>>,
}

impl Volatile {
    /// Creates an empty record.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything recorded so far, in order.
    ///
    /// For a caller asserting on what a turn wrote without reading a file back.
    /// Lines rather than events, so the times are there too.
    #[must_use]
    pub fn records(&self) -> Vec<Logged> {
        self.records.lock().expect("records lock").clone()
    }

    /// Whether a turn's end was recorded with this reason.
    #[must_use]
    pub fn ended_with(&self, reason: TurnEndReason) -> bool {
        self.records
            .lock()
            .expect("records lock")
            .iter()
            .any(|logged| {
                matches!(&logged.event, SessionEvent::TurnEnded { reason: r, .. } if *r == reason)
            })
    }
}

#[async_trait]
impl SessionLog for Volatile {
    async fn record(&self, entry: &SessionEvent) -> Result<(), RecordError> {
        self.records
            .lock()
            .expect("records lock")
            .push(Logged::now(entry.clone()));
        Ok(())
    }
}

/// What a record is, for an error message.
///
/// Named rather than printed: the failure that matters here is a record the log
/// turned down, and which record was refused is the whole of what the reader
/// needs to tell a full disk from a bad path.
fn describe(entry: &SessionEvent) -> String {
    match entry {
        SessionEvent::Session { .. } => "the session record".into(),
        SessionEvent::System { .. } => "the system instruction".into(),
        SessionEvent::Title { .. } => "the title".into(),
        SessionEvent::TurnStarted { turn_id } => format!("the start of turn {turn_id}"),
        SessionEvent::TurnEnded { turn_id, .. } => format!("the end of turn {turn_id}"),
        SessionEvent::Item { turn_id, item, .. } => {
            let kind = match item {
                ResponseItem::Message { .. } => "message",
                ResponseItem::FunctionCall { .. } => "tool call",
                ResponseItem::FunctionCallOutput { .. } => "tool result",
                ResponseItem::Reasoning { .. } => "reasoning",
            };
            format!("a {kind} in turn {turn_id}")
        }
    }
}

/// Records one history entry, with its turn and message already attached.
///
/// The loop builds one of these rather than calling [`SessionLog::record`] with a
/// hand-built entry, so the fields that must agree — the turn, and the message a
/// chunk belongs to — are filled in one place instead of at each call site.
pub fn entry_for(
    turn_id: TurnId,
    message_id: Option<MessageId>,
    item: ResponseItem,
) -> SessionEvent {
    SessionEvent::Item {
        turn_id,
        message_id,
        item,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session_event::SessionEvent;
    use crate::types::SessionId;

    /// An error that refuses everything, for the paths a real store would fail.
    #[derive(Debug, Default)]
    struct Refusing;

    #[async_trait]
    impl SessionLog for Refusing {
        async fn record(&self, _entry: &SessionEvent) -> Result<(), RecordError> {
            Err(RecordError::Io {
                what: "a message in turn t".into(),
                source: std::io::Error::other("no space left on device"),
            })
        }
    }

    /// A turn id for a test to record against.
    fn turn() -> TurnId {
        TurnId::new()
    }

    #[tokio::test]
    async fn the_volatile_log_keeps_what_it_was_given() {
        let log = Volatile::new();

        log.record(&entry_for(turn(), None, ResponseItem::assistant("hi")))
            .await
            .expect("recorded");
        log.record(&SessionEvent::TurnEnded {
            turn_id: turn(),
            reason: TurnEndReason::Completed,
        })
        .await
        .expect("recorded");

        assert_eq!(log.records().len(), 2);
    }

    #[tokio::test]
    async fn the_volatile_log_reports_turn_ends_by_reason() {
        let log = Volatile::new();
        let turn_id = turn();

        log.record(&SessionEvent::TurnStarted { turn_id })
            .await
            .expect("recorded");
        assert!(!log.ended_with(TurnEndReason::Completed), "not yet ended");

        log.record(&SessionEvent::TurnEnded {
            turn_id,
            reason: TurnEndReason::Interrupted,
        })
        .await
        .expect("recorded");

        assert!(log.ended_with(TurnEndReason::Interrupted));
        assert!(
            !log.ended_with(TurnEndReason::Completed),
            "a different reason is a different ending"
        );
    }

    #[tokio::test]
    async fn a_refused_record_is_reported_rather_than_swallowed() {
        // The failure that matters: a log that quietly stopped accepting records
        // produces a session that looks fine and cannot be recovered.
        let error = Refusing
            .record(&entry_for(turn(), None, ResponseItem::assistant("hi")))
            .await
            .expect_err("refused");

        assert!(
            error.to_string().contains("no space left on device"),
            "the cause survives: {error}"
        );
    }

    #[test]
    fn a_failure_names_the_record_it_refused() {
        // Which record was turned down is the whole of what tells a full disk
        // from a bad path, so the message has to carry it.
        let cases = [
            (
                SessionEvent::session(SessionId::new(), PathBuf::from("/w")),
                "session",
            ),
            (SessionEvent::TurnStarted { turn_id: turn() }, "turn"),
            (
                SessionEvent::TurnEnded {
                    turn_id: turn(),
                    reason: TurnEndReason::Error,
                },
                "turn",
            ),
            (
                entry_for(turn(), None, ResponseItem::assistant("x")),
                "message",
            ),
            (
                entry_for(
                    turn(),
                    None,
                    ResponseItem::FunctionCall {
                        call_id: "c1".into(),
                        name: "read".into(),
                        arguments: "{}".into(),
                    },
                ),
                "tool call",
            ),
            (
                entry_for(
                    turn(),
                    None,
                    ResponseItem::FunctionCallOutput {
                        call_id: "c1".into(),
                        output: "text".into(),
                        is_error: false,
                    },
                ),
                "tool result",
            ),
            (
                entry_for(
                    turn(),
                    None,
                    ResponseItem::Reasoning {
                        content: "x".into(),
                    },
                ),
                "reasoning",
            ),
        ];

        for (entry, expected) in cases {
            let described = describe(&entry);
            assert!(
                described.contains(expected),
                "expected {expected:?} in {described:?}"
            );
        }
    }

    #[test]
    fn an_entry_carries_the_turn_and_message_it_was_given() {
        // Built in one place so the fields that must agree cannot drift per call
        // site — a record naming the wrong turn is not obviously wrong on read.
        let turn_id = turn();
        let message_id = MessageId::new();

        let entry = entry_for(
            turn_id,
            Some(message_id.clone()),
            ResponseItem::assistant("hi"),
        );

        assert_eq!(entry.turn_id(), Some(turn_id));
        match entry {
            SessionEvent::Item {
                message_id: recorded,
                ..
            } => assert_eq!(recorded, Some(message_id)),
            other => panic!("expected an item: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_real_writer_satisfies_the_same_trait() {
        // The seam is only worth having if the production type goes through it.
        let path = std::env::temp_dir().join(format!(
            "srud-session-log-trait-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let writer = crate::session_store::SessionLogWriter::create(&path)
            .await
            .expect("a log to open");

        writer
            .record(&entry_for(turn(), None, ResponseItem::assistant("hi")))
            .await
            .expect("recorded");

        let log = crate::session_store::read(&path)
            .await
            .expect("a log to read");
        assert_eq!(log.entries.len(), 1, "the record reached the file");
        let _ = std::fs::remove_file(&path);
    }
}
