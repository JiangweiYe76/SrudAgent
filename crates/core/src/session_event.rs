//! One session event: what a session's durable log holds.
//!
//! One JSON object per line, appended. The log is the authority for a session —
//! the in-memory history and any index are projections of it, and a restart
//! rebuilds them by reading it back.
//!
//! # Why not the history items themselves
//!
//! [`ResponseItem`] is what the model sees, and it is not what the log needs to
//! hold:
//!
//! - **The log carries more than the wire does.** A turn's start and end have no
//!   `session/update` representation — the end closes the `session/prompt`
//!   response instead of streaming — but recovery needs both recorded, or a log
//!   that stops mid-turn cannot say whether it did.
//! - **History items carry no id.** Locating one item inside a session needs the
//!   turn it belongs to, and grouping the chunks of one message needs a message
//!   id. Neither exists on [`ResponseItem`].
//!
//! # Shape
//!
//! ```text
//! {"time":"2026-10-03T21:28:14.123Z","event":{"type":"session","session_id":"…","cwd":"…","version":1}}
//! {"time":"…","event":{"type":"system","content":"# Role\n\n…"}}
//! {"time":"…","event":{"type":"title","title":"Fix the timestamp"}}
//! {"time":"…","event":{"type":"turn_started","turn_id":"…"}}
//! {"time":"…","event":{"type":"item","turn_id":"…","message_id":"…","item":{…}}}
//! {"time":"…","event":{"type":"turn_ended","turn_id":"…","reason":"interrupted"}}
//! ```
//!
//! `version` is on the first record rather than in the filename: a reader has to
//! open the file to learn how to read it, so that is where the answer belongs.
//!
//! # What is in an `Item`
//!
//! Anything that enters the model's history, recorded as it enters it — which
//! includes content that is not conversation. The env-context block is a
//! [`ResponseItem::Message`] the user never typed, and it is recorded as one,
//! recognised by the same [`crate::context::is_item`] sniff the runtime uses.
//! Keeping it ordinary means the log has no case for it; the cost is that a
//! reader wanting to tell context from conversation has to look at the content.
//!
//! The system instruction is also not an `Item`, and is recorded once as
//! [`SessionEvent::System`] above the turns instead. The title is likewise its
//! own record, since it changes after the session is written.

use std::path::PathBuf;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{ResponseItem, TurnEndReason, TurnId};

/// Identifies one model's message within a turn.
///
/// The same value on every chunk of that message, so a consumer can tell where
/// one message ends and the next begins. A streaming client accumulates by id; a
/// reader of the log groups by it to rebuild the same message.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub String);

impl MessageId {
    /// Mints a fresh, random id.
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The format version written into every new log.
///
/// Bumped when a change makes older logs unreadable or ambiguous. A reader that
/// finds a version it does not know must not guess: the records it cannot
/// interpret are skipped rather than assumed, so an unknown version degrades to
/// a partially-read log instead of a wrong one.
pub const FORMAT_VERSION: u32 = 1;

/// One thing that happened in a session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    /// The session's first record. Written once, when the session is created.
    Session {
        /// The session this log belongs to.
        session_id: crate::types::SessionId,
        /// The directory the session works in.
        cwd: PathBuf,
        /// The [`FORMAT_VERSION`] this log was written at.
        version: u32,
    },

    /// The system instruction, written once above the turns.
    ///
    /// Not an [`Item`](Self::Item): in a model request this is a field beside the
    /// conversation rather than a message inside it, so recording it as one would
    /// leave a reader lifting it back out before it could rebuild a request. How
    /// it reaches the wire — a `role: system` message, or a top-level field — is
    /// the adapter's business, and the two adapters differ.
    ///
    /// Written once because it is constant, and constant because a provider
    /// caches a request by its prefix: an instruction that moved would cost the
    /// cache on every request. A reader rebuilding a request takes the last of
    /// these, which is what lets a record of a *changed* instruction — a tool
    /// removed, a skill unloaded — be added later without touching this format.
    System {
        /// The rendered instruction, as sent.
        content: String,
    },

    /// The session's display title, as last set.
    ///
    /// Written whenever the title changes — the first prompt naming the session,
    /// or a client renaming it — and a reader takes the last one. `None` is a
    /// title cleared, which is not the same as no title record at all.
    ///
    /// Its own record rather than a field on [`Session`](Self::Session): a title
    /// changes after the session exists, and an append-only log cannot go back and
    /// edit the first line.
    Title {
        /// The title, or `None` when it was cleared.
        title: Option<String>,
    },

    /// A turn began. Has no wire counterpart.
    TurnStarted {
        /// Which turn.
        turn_id: TurnId,
    },

    /// One entry of the recorded history.
    ///
    /// Recorded before the event that announces it, so anything a consumer has
    /// seen is already durable.
    Item {
        /// The turn this belongs to.
        turn_id: TurnId,
        /// Which message, for the entries that are one. `None` for a tool call,
        /// which is identified by the call id in the item itself.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<MessageId>,
        /// The entry itself.
        item: ResponseItem,
    },

    /// A turn ended.
    ///
    /// A turn that was cut short — interrupted, or lost to a crash — is recorded
    /// here when the log is next read, so a log never stops in the middle of a
    /// turn without saying why.
    TurnEnded {
        /// Which turn.
        turn_id: TurnId,
        /// Why it stopped.
        reason: TurnEndReason,
    },
}

/// One line of the log: an event, and when it was written.
///
/// The time sits beside the event rather than inside each of its variants, so
/// every record carries one without the variants declaring it — and so
/// [`SessionEvent`] stays a value that serialises on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Logged {
    /// When the line was written.
    ///
    /// Millisecond precision, fixed, so a log read as text is also in
    /// chronological order: a variable-width timestamp does not sort as text once
    /// one line carries a fractional second and another does not.
    #[serde(serialize_with = "write_millis")]
    pub time: DateTime<Utc>,
    /// What happened.
    pub event: SessionEvent,
}

impl Logged {
    /// Stamps an event with the moment it is written.
    ///
    /// Truncated where the clock is read rather than where the line is
    /// serialised: the log holds milliseconds, and a stand-in keeping nanoseconds
    /// would disagree with the file it stands in for.
    #[must_use]
    pub fn now(event: SessionEvent) -> Self {
        Self {
            time: DateTime::from_timestamp_millis(Utc::now().timestamp_millis())
                .expect("a millisecond timestamp is in range"),
            event,
        }
    }
}

/// Writes a timestamp as UTC with fixed millisecond precision.
fn write_millis<S>(value: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

impl SessionEvent {
    /// Builds the session's first record.
    #[must_use]
    pub fn session(session_id: crate::types::SessionId, cwd: PathBuf) -> Self {
        Self::Session {
            session_id,
            cwd,
            version: FORMAT_VERSION,
        }
    }

    /// Builds the record of the system instruction.
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self::System {
            content: content.into(),
        }
    }

    /// Builds a title change. `None` records a cleared title.
    #[must_use]
    pub fn title(title: Option<String>) -> Self {
        Self::Title { title }
    }

    /// Builds a history-entry record.
    #[must_use]
    pub fn item(turn_id: TurnId, message_id: Option<MessageId>, item: ResponseItem) -> Self {
        Self::Item {
            turn_id,
            message_id,
            item,
        }
    }

    /// The turn this record belongs to, if it belongs to one.
    ///
    /// The session's own record, the system instruction and the title do not: all
    /// three are written at session scope, outside any turn.
    #[must_use]
    pub fn turn_id(&self) -> Option<TurnId> {
        match self {
            Self::Session { .. } | Self::System { .. } | Self::Title { .. } => None,
            Self::TurnStarted { turn_id }
            | Self::Item { turn_id, .. }
            | Self::TurnEnded { turn_id, .. } => Some(*turn_id),
        }
    }

    /// The history entry this record carries, if it carries one.
    #[must_use]
    pub fn history_item(&self) -> Option<&ResponseItem> {
        match self {
            Self::Item { item, .. } => Some(item),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::types::{Role, SessionId};

    /// A fixed instant, so a test asserting on a rendered time has one to assert
    /// against.
    fn when() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 3, 15, 7, 23).unwrap()
    }

    #[test]
    fn the_first_record_names_the_session_and_the_format() {
        let entry = SessionEvent::session(SessionId::new(), PathBuf::from("/w"));
        let value = serde_json::to_value(&entry).unwrap();

        assert_eq!(value["type"], "session");
        assert_eq!(value["cwd"], "/w");
        assert_eq!(value["version"], FORMAT_VERSION);
    }

    #[test]
    fn a_line_carries_the_time_beside_the_event() {
        // The shape a reader depends on: the event keeps its own tag, the time is
        // fixed-width UTC beside it, and a line read back is the line written.
        let line = Logged::now(SessionEvent::system("# Role"));
        let value = serde_json::to_value(&line).unwrap();

        assert_eq!(value["event"]["type"], "system");
        assert_eq!(value["event"]["content"], "# Role");
        let time = value["time"].as_str().unwrap();
        assert_eq!(
            time.len(),
            "2026-10-03T21:28:14.123Z".len(),
            "fixed millisecond precision, so lines sort as text: {time}"
        );
        assert!(time.ends_with('Z'), "UTC, and the offset says so: {time}");

        let back: Logged = serde_json::from_value(value).unwrap();
        assert_eq!(back, line, "a line read back is the line written");
    }

    #[test]
    fn every_record_round_trips_through_json() {
        let turn_id = TurnId::new();
        let entries = vec![
            SessionEvent::session(SessionId::new(), PathBuf::from("/w")),
            SessionEvent::system("# Role\n\nYou are Srud."),
            SessionEvent::title(Some("Fix the timestamp".into())),
            SessionEvent::TurnStarted { turn_id },
            SessionEvent::item(
                turn_id,
                Some(MessageId::new()),
                ResponseItem::assistant("hi"),
            ),
            // A tool call has no message id: its call id identifies it.
            SessionEvent::item(
                turn_id,
                None,
                ResponseItem::FunctionCall {
                    call_id: "call_1".into(),
                    name: "read".into(),
                    arguments: r#"{"path":"/etc/hosts"}"#.into(),
                },
            ),
            SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Interrupted,
            },
        ];

        for entry in entries {
            let json = serde_json::to_string(&entry).expect("serialises");
            let back: SessionEvent = serde_json::from_str(&json).expect("deserialises");
            assert_eq!(back, entry, "did not round-trip: {json}");
        }
    }

    #[test]
    fn the_tag_is_what_names_each_record() {
        // The reader dispatches on this, so it has to be present and stable, and
        // the fields beside it are what a reader then reads.
        let turn_id = TurnId::new();
        let cases = [
            (
                SessionEvent::system("# Role\n\nYou are Srud."),
                "system",
                vec!["content"],
            ),
            (
                SessionEvent::title(Some("Fix the timestamp".into())),
                "title",
                vec!["title"],
            ),
            (
                SessionEvent::TurnStarted { turn_id },
                "turn_started",
                vec!["turn_id"],
            ),
            (
                SessionEvent::TurnEnded {
                    turn_id,
                    reason: TurnEndReason::Completed,
                },
                "turn_ended",
                vec!["turn_id", "reason"],
            ),
            (
                SessionEvent::item(turn_id, None, ResponseItem::assistant("x")),
                "item",
                vec!["turn_id", "item"],
            ),
        ];

        for (entry, tag, fields) in cases {
            let value = serde_json::to_value(&entry).unwrap();
            assert_eq!(value["type"], tag);

            // The tag plus exactly the fields this variant carries: no more, so a
            // reader can tell the variants apart without consulting the schema,
            // and no fewer, so nothing is silently dropped on the way to disk.
            let object = value.as_object().unwrap();
            let mut keys: Vec<_> = object.keys().map(String::as_str).collect();
            keys.sort_unstable();
            let mut expected = vec!["type"];
            expected.extend(fields);
            expected.sort_unstable();
            assert_eq!(keys, expected, "{value}");
        }
    }

    #[test]
    fn a_record_without_a_message_id_omits_the_field() {
        // Written rather than null: an absent field is the statement "this is not
        // part of a message", and `null` would be a different claim.
        let entry = SessionEvent::item(TurnId::new(), None, ResponseItem::assistant("x"));
        let json = serde_json::to_string(&entry).unwrap();

        assert!(!json.contains("message_id"), "{json}");
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.history_item(), Some(&ResponseItem::assistant("x")));
    }

    #[test]
    fn a_record_at_session_scope_belongs_to_no_turn() {
        // The reader reconstructs a session by walking records in order, so it
        // has to be able to tell which records open a turn. These are written at
        // session scope and must not be attributed to one.
        for outside in [
            SessionEvent::session(SessionId::new(), PathBuf::from("/w")),
            SessionEvent::system("# Role"),
            SessionEvent::title(Some("A title".into())),
            SessionEvent::title(None),
        ] {
            assert_eq!(outside.turn_id(), None, "{outside:?}");
        }

        let turn_id = TurnId::new();
        assert_eq!(
            SessionEvent::TurnStarted { turn_id }.turn_id(),
            Some(turn_id)
        );
        assert_eq!(
            SessionEvent::TurnEnded {
                turn_id,
                reason: TurnEndReason::Completed
            }
            .turn_id(),
            Some(turn_id)
        );
    }

    #[test]
    fn a_cleared_title_is_written_rather_than_omitted() {
        // `null` is the statement "this session has no title". Omitting the field
        // would read as "this record says nothing about the title", which under
        // latest-wins would leave the previous one standing.
        let value = serde_json::to_value(SessionEvent::title(None)).unwrap();

        assert_eq!(value["type"], "title");
        assert!(value["title"].is_null(), "{value}");
        let back: SessionEvent = serde_json::from_value(value).unwrap();
        assert_eq!(back, SessionEvent::title(None));
    }

    #[test]
    fn a_message_id_is_stable_across_serialisation() {
        // The whole point of the id: every chunk of one message repeats it, so a
        // consumer can group them. A value that changed per chunk would group
        // nothing.
        let id = MessageId::new();
        let first = SessionEvent::item(
            TurnId::new(),
            Some(id.clone()),
            ResponseItem::assistant("a"),
        );
        let second = SessionEvent::item(
            TurnId::new(),
            Some(id.clone()),
            ResponseItem::assistant("b"),
        );

        let ids: Vec<_> = [&first, &second]
            .iter()
            .map(|entry| {
                serde_json::to_value(entry).unwrap()["message_id"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(ids[0], ids[1], "the same message keeps one id");
    }

    #[test]
    fn the_env_context_block_survives_as_ordinary_content() {
        // It is recorded as a user message, so the log needs no case for it: it
        // round-trips as the content it is, and the reader recognises it the
        // same way the runtime does.
        let entry = SessionEvent::item(
            TurnId::new(),
            None,
            ResponseItem::Message {
                role: Role::User,
                content: crate::context::render(std::path::Path::new("/w"), when().into()),
            },
        );

        let json = serde_json::to_string(&entry).unwrap();
        let back: SessionEvent = serde_json::from_str(&json).unwrap();
        let item = back.history_item().unwrap();

        assert!(
            crate::context::is_item(item),
            "recognised after a round-trip"
        );
    }
}
