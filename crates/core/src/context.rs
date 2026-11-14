//! The env-context blocks: where a turn runs, and what has changed since.
//!
//! Reported as user messages immediately ahead of the user's own input, so the
//! model reads its surroundings before it reads the request. They are contextual,
//! not conversational: nothing emits an event for them, and they are recognised
//! by their tags rather than by where they sit in history.
//!
//! `<env-context>` opens the first turn with the directory and the local time.
//! `<env-context-update>` appears before a later turn's input when the system
//! instruction no longer describes the session — a skill or server came or went,
//! or the date has moved on. There is no block for a turn where nothing changed.
//!
//! The time is the user's local time: that is the clock the user's own words —
//! "today", "this afternoon", deadlines — are measured on.

use std::path::Path;

use chrono::{DateTime, Local, NaiveDateTime, TimeZone};

use crate::prompt::ContextChange;
use crate::types::{ResponseItem, Role};

/// The tag opening a block. [`is_item`] recognises a block by this text, so it
/// has to be exactly what [`render`] writes.
pub const OPEN_TAG: &str = "<env-context>";

/// The tag closing the same block.
pub const CLOSE_TAG: &str = "</env-context>";

/// The tag opening a change block.
pub const UPDATE_OPEN_TAG: &str = "<env-context-update>";

/// The tag closing a change block.
pub const UPDATE_CLOSE_TAG: &str = "</env-context-update>";

/// The date format: local date and wall-clock time to the minute.
const DATE_FORMAT: &str = "%Y-%m-%d %H:%M";

/// Renders the block for `cwd` as of `now`.
#[must_use]
pub fn render(cwd: &Path, now: DateTime<Local>) -> String {
    let cwd = escape(&cwd.display().to_string());
    format!(
        "{OPEN_TAG}\n  <cwd>{cwd}</cwd>\n  <date>{}</date>\n{CLOSE_TAG}",
        now.format(DATE_FORMAT)
    )
}

/// Builds the history item for `cwd`, stamped with the current local time.
#[must_use]
pub fn item(cwd: &Path) -> ResponseItem {
    ResponseItem::user(render(cwd, Local::now()))
}

/// Renders what changed since the instruction settled.
///
/// Only the parts that changed are written: a new date, the skills that came or
/// went, and the servers that came or went. An addition carries what the
/// instruction would have said about it — a skill's description, a server's
/// notes — because nothing else tells the model how to reach it. A removal needs
/// only the name. `changes` is expected to be non-empty: a turn with nothing to
/// report writes no block at all.
#[must_use]
pub fn render_update(changes: &[ContextChange]) -> String {
    let mut lines = Vec::with_capacity(changes.len());
    for change in changes {
        match change {
            ContextChange::Time { now } => {
                lines.push(format!("  <date>{}</date>", now.format(DATE_FORMAT)));
            }
            ContextChange::SkillAdded { name, description } => {
                lines.push(format!(
                    "  <skill-added>{}: {}</skill-added>",
                    escape(name),
                    description
                ));
            }
            ContextChange::SkillRemoved(name) => {
                lines.push(format!("  <skill-removed>{}</skill-removed>", escape(name)));
            }
            ContextChange::ServerAdded { name, instructions } => {
                lines.push(format!(
                    "  <server-added>{}: {}</server-added>",
                    escape(name),
                    instructions
                ));
            }
            ContextChange::ServerRemoved(name) => {
                lines.push(format!(
                    "  <server-removed>{}</server-removed>",
                    escape(name)
                ));
            }
        }
    }
    format!(
        "{UPDATE_OPEN_TAG}\n{}\n{UPDATE_CLOSE_TAG}",
        lines.join("\n")
    )
}

/// Builds the history item that tells the model what changed.
#[must_use]
pub fn update_item(changes: &[ContextChange]) -> ResponseItem {
    ResponseItem::user(render_update(changes))
}

/// Whether an item is a context block rather than something the user said.
#[must_use]
pub fn is_item(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::Message { role: Role::User, content }
            if content.starts_with(OPEN_TAG) || content.starts_with(UPDATE_OPEN_TAG)
    )
}

/// The date the last context block told the model, when the history holds one.
///
/// Read when a session asks whether the date it was given has gone stale: the
/// block is recorded once, so the last one in history is the last date the model
/// was told.
#[must_use]
pub fn last_time(history: &[ResponseItem]) -> Option<DateTime<Local>> {
    history.iter().rev().find_map(|item| {
        if !is_item(item) {
            return None;
        }
        let ResponseItem::Message { content, .. } = item else {
            return None;
        };
        let (_, rest) = content.split_once("<date>")?;
        let (date, _) = rest.split_once("</date>")?;
        let naive = NaiveDateTime::parse_from_str(date.trim(), DATE_FORMAT).ok()?;
        Local.from_local_datetime(&naive).single()
    })
}

/// Escapes the characters that would otherwise read as markup.
///
/// A path is whatever the host gave us, and `&` is legal in one, so the value
/// has to be escaped rather than assumed to be plain.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32, minute: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 10, 2, hour, minute, 0)
            .single()
            .expect("unambiguous local time")
    }

    #[test]
    fn the_block_carries_the_directory_and_the_local_time() {
        let rendered = render(Path::new("/work/ws"), at(14, 35));
        assert_eq!(
            rendered,
            "<env-context>\n  <cwd>/work/ws</cwd>\n  <date>2026-10-02 14:35</date>\n</env-context>"
        );
    }

    #[test]
    fn the_time_is_to_the_minute() {
        let rendered = render(Path::new("/work"), at(9, 5));
        assert!(
            rendered.contains("<date>2026-10-02 09:05</date>"),
            "minutes are zero-padded: {rendered}"
        );
    }

    #[test]
    fn markup_in_a_path_is_escaped() {
        let rendered = render(Path::new("/a&b/<c>"), at(0, 0));
        assert!(
            rendered.contains("<cwd>/a&amp;b/&lt;c&gt;</cwd>"),
            "a path cannot end the element early: {rendered}"
        );
    }

    #[test]
    fn the_item_is_a_user_message_tagged_as_context() {
        let item = item(Path::new("/work"));
        assert!(is_item(&item));
        assert!(matches!(
            item,
            ResponseItem::Message {
                role: Role::User,
                ..
            }
        ));
    }

    #[test]
    fn a_users_own_message_is_not_context() {
        assert!(!is_item(&ResponseItem::user("hello")));
        assert!(!is_item(&ResponseItem::assistant("<env-context>")));
    }

    #[test]
    fn the_last_block_says_when_the_model_was_last_told() {
        let at = at(14, 35);
        let history = vec![
            ResponseItem::user(render(Path::new("/work/ws"), at)),
            ResponseItem::user("hello"),
        ];

        assert_eq!(last_time(&history), Some(at));
        assert_eq!(
            last_time(&[ResponseItem::user("hello")]),
            None,
            "a user's own words are not a date"
        );
    }

    #[test]
    fn an_update_names_only_what_changed() {
        let changes = [
            ContextChange::Time { now: at(15, 5) },
            ContextChange::SkillAdded {
                name: "gamma".into(),
                description: "third".into(),
            },
            ContextChange::ServerAdded {
                name: "remote".into(),
                instructions: "call it".into(),
            },
            ContextChange::ServerRemoved("docs".into()),
        ];

        assert_eq!(
            render_update(&changes),
            "<env-context-update>\n  <date>2026-10-02 15:05</date>\n  \
             <skill-added>gamma: third</skill-added>\n  \
             <server-added>remote: call it</server-added>\n  \
             <server-removed>docs</server-removed>\n</env-context-update>"
        );
    }

    #[test]
    fn an_update_is_context_rather_than_something_the_user_said() {
        let item = update_item(&[ContextChange::SkillAdded {
            name: "gamma".into(),
            description: "third".into(),
        }]);

        assert!(is_item(&item), "a reader skipping context skips this too");
    }
}
