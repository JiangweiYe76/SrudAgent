//! Conversion between the core runtime's [`Event`]s and ACP wire types.
//!
//! Two directions live here:
//!
//! - **Outbound** ([`to_session_update`], [`to_notification`]): every core
//!   `Event` becomes a standard `SessionUpdate` variant, with the turn id
//!   attached under `_meta.srud` so SrudAgent's own frontend can target
//!   interrupts and index by turn. `TurnStarted` and `TurnComplete` are not
//!   updates — the former has no wire analogue, the latter closes the
//!   `session/prompt` response instead of streaming.
//! - **Inbound** ([`prompt_text`]): an ACP prompt is a `ContentBlock` array,
//!   but the runtime takes plain text, so the blocks are concatenated and
//!   non-text content is dropped (the agent declares text-only prompt
//!   capabilities).

use srud_core::types::{Event, TurnEndReason};
use srud_protocol::acp::{
    AcpError, ContentBlock, ContentChunk, Meta, Notification, SessionId, SessionNotification,
    SessionUpdate, TextContent, ToolCall, ToolCallContent, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, CLIENT_METHOD_NAMES,
};
use srud_protocol::srud::meta::SrudMeta;
use srud_protocol::srud::turn_end::{map_turn_end_reason, TurnEndWire};

/// Which core events have no `session/update` representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// The turn began; there is nothing to render yet.
    TurnStarted,
    /// The turn ended; the outcome flows through the prompt response.
    TurnComplete,
}

/// Converts one core `Event` into its ACP `SessionUpdate`.
///
/// The turn id is not attached here — [`to_notification`] carries it on the
/// outer `SessionNotification`, since not every update variant has a `meta`
/// field of its own.
#[must_use]
pub fn to_session_update(event: Event) -> Option<Result<SessionUpdate, Skipped>> {
    let update = match event {
        Event::TurnStarted { .. } => return Some(Err(Skipped::TurnStarted)),
        Event::UserMessage { content } => SessionUpdate::UserMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(content)),
        )),
        Event::AgentMessageDelta { delta } => SessionUpdate::AgentMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(delta)),
        )),
        Event::AgentThoughtDelta { delta } => SessionUpdate::AgentThoughtChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(delta)),
        )),
        Event::ToolCallBegin {
            call_id,
            name,
            arguments,
        } => SessionUpdate::ToolCall(
            ToolCall::new(call_id, name.clone())
                .name(name)
                .status(ToolCallStatus::InProgress)
                .raw_input(arguments),
        ),
        Event::ToolCallEnd {
            call_id, result, ..
        } => {
            let status = if result.is_error {
                ToolCallStatus::Failed
            } else {
                ToolCallStatus::Completed
            };
            SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                call_id,
                ToolCallUpdateFields::new()
                    .status(status)
                    .content(vec![ToolCallContent::Content(
                        srud_protocol::acp::ToolCallContentBlock::new(ContentBlock::Text(
                            TextContent::new(result.output),
                        )),
                    )])
                    .raw_output(serde_json::json!({ "is_error": result.is_error })),
            ))
        }
        Event::TurnComplete { .. } => return Some(Err(Skipped::TurnComplete)),
    };
    Some(Ok(update))
}

/// Builds the `session/update` notification for a core event.
///
/// Returns `None` for events that have no wire representation (see
/// [`Skipped`]).
#[must_use]
pub fn to_notification(
    session_id: &SessionId,
    turn_id: &str,
    event: Event,
) -> Option<Notification<SessionNotification>> {
    let update = match to_session_update(event)? {
        Ok(update) => update,
        Err(_) => return None,
    };
    let (update, outer_meta) = attach_turn_meta(update, turn_id);
    let notification = SessionNotification::new(session_id.clone(), update).meta(outer_meta);
    Some(Notification {
        method: CLIENT_METHOD_NAMES.session_update.into(),
        params: Some(notification),
    })
}

/// Attaches `_meta.srud.turnId` to an update.
///
/// The turn id goes on the **outer** `SessionNotification` rather than the
/// inner update variant, because several `SessionUpdate` variants do not carry
/// a `meta` field of their own, and a single placement keeps the reader
/// uniform.
fn attach_turn_meta(update: SessionUpdate, turn_id: &str) -> (SessionUpdate, Meta) {
    let srud = SrudMeta {
        turn_id: Some(turn_id.to_string()),
        ..Default::default()
    };
    let meta = srud
        .to_meta()
        .expect("a payload with turnId is never empty");
    (update, meta)
}

/// Concatenates the text of a prompt's content blocks.
///
/// Non-text blocks are dropped here; the dispatch layer validates the prompt
/// against the declared capabilities before calling this, so dropping is a
/// no-op for compliant clients.
#[must_use]
pub fn prompt_text(blocks: &[ContentBlock]) -> String {
    let mut text = String::new();
    for block in blocks {
        if let ContentBlock::Text(content) = block {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&content.text);
        }
    }
    text
}

/// Maps a turn's end reason onto the `session/prompt` outcome.
///
/// `Error` has no result to carry — it becomes a JSON-RPC error, matching the
/// protocol crate's mapping table where only the three result-bearing reasons
/// flow through `map_turn_end_reason`. The returned [`TurnEndWire`] may carry
/// a `_meta.srud` marker (e.g. the blocked reason) for the response.
pub fn prompt_outcome(reason: TurnEndReason) -> Result<TurnEndWire, AcpError> {
    use srud_protocol::srud::turn_end::SrudTurnEndReason;
    let srud = match reason {
        TurnEndReason::Completed => SrudTurnEndReason::Completed,
        TurnEndReason::Interrupted => SrudTurnEndReason::Interrupted,
        TurnEndReason::Blocked => SrudTurnEndReason::Blocked,
        TurnEndReason::Error => {
            return Err(AcpError::new(
                srud_protocol::error::INTERNAL_ERROR,
                "the turn ended with an internal error",
            ))
        }
    };
    Ok(map_turn_end_reason(srud))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use srud_core::tools::ToolOutcome;

    fn session_id() -> SessionId {
        SessionId::new("s-1")
    }

    fn notify(event: Event) -> Notification<SessionNotification> {
        to_notification(&session_id(), "turn-1", event).expect("event maps to a notification")
    }

    #[test]
    fn turn_started_and_complete_are_not_updates() {
        use srud_core::types::TurnId;
        let turn_id = TurnId::new();
        assert_eq!(
            to_session_update(Event::TurnStarted { turn_id }),
            Some(Err(Skipped::TurnStarted))
        );
        assert_eq!(
            to_session_update(Event::TurnComplete {
                turn_id,
                reason: TurnEndReason::Completed,
            }),
            Some(Err(Skipped::TurnComplete))
        );
        assert!(to_notification(&session_id(), "t", Event::TurnStarted { turn_id }).is_none());
    }

    #[test]
    fn message_deltas_map_to_chunk_variants() {
        let n = notify(Event::AgentMessageDelta {
            delta: "hello".into(),
        });
        assert!(matches!(
            n.params.as_ref().unwrap().update,
            SessionUpdate::AgentMessageChunk(_)
        ));
        let n = notify(Event::AgentThoughtDelta {
            delta: "think".into(),
        });
        assert!(matches!(
            n.params.as_ref().unwrap().update,
            SessionUpdate::AgentThoughtChunk(_)
        ));
        let n = notify(Event::UserMessage {
            content: "hi".into(),
        });
        assert!(matches!(
            n.params.as_ref().unwrap().update,
            SessionUpdate::UserMessageChunk(_)
        ));
    }

    #[test]
    fn tool_call_begin_carries_input_and_in_progress_status() {
        let n = notify(Event::ToolCallBegin {
            call_id: "c1".into(),
            name: "echo".into(),
            arguments: json!({ "text": "hi" }),
        });
        let SessionUpdate::ToolCall(call) = &n.params.as_ref().unwrap().update else {
            panic!("expected ToolCall");
        };
        assert_eq!(call.title, "echo");
        assert_eq!(call.name.as_deref(), Some("echo"));
        assert_eq!(call.status, ToolCallStatus::InProgress);
        assert_eq!(call.raw_input, Some(json!({ "text": "hi" })));
    }

    #[test]
    fn tool_call_end_maps_outcome_to_status_and_content() {
        let n = notify(Event::ToolCallEnd {
            call_id: "c1".into(),
            name: "echo".into(),
            result: ToolOutcome::success("out"),
        });
        let SessionUpdate::ToolCallUpdate(update) = &n.params.as_ref().unwrap().update else {
            panic!("expected ToolCallUpdate");
        };
        assert_eq!(update.fields.status, Some(ToolCallStatus::Completed));
        assert_eq!(update.fields.raw_output, Some(json!({ "is_error": false })));

        let n = notify(Event::ToolCallEnd {
            call_id: "c1".into(),
            name: "echo".into(),
            result: ToolOutcome::failure("boom"),
        });
        let SessionUpdate::ToolCallUpdate(update) = &n.params.as_ref().unwrap().update else {
            panic!("expected ToolCallUpdate");
        };
        assert_eq!(update.fields.status, Some(ToolCallStatus::Failed));
    }

    #[test]
    fn notification_carries_turn_id_under_meta_srud() {
        let n = notify(Event::AgentMessageDelta { delta: "x".into() });
        let value = serde_json::to_value(&n).unwrap();
        assert_eq!(value["method"], json!("session/update"));
        assert_eq!(value["params"]["sessionId"], json!("s-1"));
        assert_eq!(value["params"]["_meta"]["srud"]["turnId"], json!("turn-1"));
        assert_eq!(
            value["params"]["update"]["sessionUpdate"],
            json!("agent_message_chunk")
        );
    }

    #[test]
    fn prompt_text_joins_text_blocks_only() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Text(TextContent::new("b")),
        ];
        assert_eq!(prompt_text(&blocks), "a\nb");
        assert_eq!(prompt_text(&[]), "");
    }

    #[test]
    fn prompt_outcome_matches_the_mapping_table() {
        use srud_protocol::acp::StopReason;
        let wire = prompt_outcome(TurnEndReason::Completed).unwrap();
        assert_eq!(wire.stop_reason, StopReason::EndTurn);
        assert_eq!(wire.meta, None);
        let wire = prompt_outcome(TurnEndReason::Interrupted).unwrap();
        assert_eq!(wire.stop_reason, StopReason::Cancelled);
        let wire = prompt_outcome(TurnEndReason::Blocked).unwrap();
        assert_eq!(wire.stop_reason, StopReason::EndTurn);
        assert_eq!(
            wire.meta
                .as_ref()
                .and_then(|m| m.turn_end_reason.as_deref()),
            Some("blocked")
        );
        let err = prompt_outcome(TurnEndReason::Error).unwrap_err();
        assert_eq!(i32::from(err.code), srud_protocol::error::INTERNAL_ERROR);
    }
}
