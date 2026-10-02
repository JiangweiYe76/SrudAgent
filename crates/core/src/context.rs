//! The env-context block: where and when a turn runs.
//!
//! Recorded as a user message immediately ahead of the user's own input, so the
//! model reads where it is and what time it is before it reads the request. It
//! is contextual, not conversational: nothing emits an event for it, and it is
//! recognised by its tag rather than by where it sits in history.
//!
//! The time is the user's local time: that is the clock the user's own words —
//! "today", "this afternoon", deadlines — are measured on.

use std::path::Path;

use chrono::{DateTime, Local};

use crate::types::{ResponseItem, Role};

/// The tag opening a block. [`is_item`] recognises a block by this text, so it
/// has to be exactly what [`render`] writes.
pub const OPEN_TAG: &str = "<env-context>";

/// The tag closing the same block.
pub const CLOSE_TAG: &str = "</env-context>";

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

/// Whether an item is an env-context block rather than something the user said.
#[must_use]
pub fn is_item(item: &ResponseItem) -> bool {
    matches!(
        item,
        ResponseItem::Message { role: Role::User, content } if content.starts_with(OPEN_TAG)
    )
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
}
