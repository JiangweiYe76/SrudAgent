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
//! - **Inbound** ([`prompt_parts`]): an ACP prompt is a
//!   `ContentBlock` array, split into the text and images a turn can carry.

use srud_core::client::TokenUsage;
use srud_core::session_event::SessionEvent;
use srud_core::types::{Event, ResponseItem, Role, TurnEndReason};
use srud_protocol::acp::{
    AcpError, ContentBlock, ContentChunk, ImageContent, Meta, Notification, SessionId,
    SessionInfoUpdate, SessionNotification, SessionUpdate, TextContent, ToolCall, ToolCallContent,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, UsageUpdate, CLIENT_METHOD_NAMES,
};
use srud_protocol::error::INVALID_PARAMS;
use srud_protocol::srud::meta::{SrudMeta, SrudUsage};
use srud_protocol::srud::turn_end::{map_turn_end_reason, TurnEndWire};

/// Which core events have no `session/update` representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// The turn began; there is nothing to render yet.
    TurnStarted,
    /// The turn ended; the outcome flows through the prompt response.
    TurnComplete,
    /// The turn's answer was taken back; it is carried by `turnDiscarded` on the
    /// outer `_meta`, so the update itself has nothing to say.
    TurnDiscarded,
    /// The provider reported usage, but the session's model has no configured
    /// context window, so there is no `size` for a `usage` update to report
    /// against. The breakdown has nowhere else to ride.
    NoContextWindow,
}

/// Converts one core `Event` into its ACP `SessionUpdate`.
///
/// The turn id is not attached here — [`to_notification`] carries it on the
/// outer `SessionNotification`, since not every update variant has a `meta`
/// field of its own.
///
/// `context_size` is the configured window of the session's model: a usage
/// report without one to compare against has no wire representation.
#[must_use]
pub fn to_session_update(
    event: Event,
    context_size: Option<u64>,
) -> Option<Result<SessionUpdate, Skipped>> {
    let update = match event {
        Event::TurnStarted { .. } => return Some(Err(Skipped::TurnStarted)),
        Event::UserMessage { content } => SessionUpdate::UserMessageChunk(ContentChunk::new(
            ContentBlock::Text(TextContent::new(content)),
        )),
        // The message id rides on the chunk: it is what tells a consumer where
        // one message ends and the next begins, which the text of a fragment
        // cannot say. Omitted rather than null when absent, because absent is the
        // claim "this is not part of a message".
        Event::AgentMessageDelta { message_id, delta } => {
            SessionUpdate::AgentMessageChunk(chunk(delta, message_id))
        }
        Event::AgentThoughtDelta { message_id, delta } => {
            SessionUpdate::AgentThoughtChunk(chunk(delta, message_id))
        }
        Event::ToolCallNamed { call_id, name } => tool_call_named(call_id, name),
        Event::ToolCallBegin {
            call_id,
            name,
            arguments,
        } => tool_call_input(call_id, name, arguments),
        Event::ToolCallEnd {
            call_id, result, ..
        } => tool_result(call_id, result.output, result.is_error),
        Event::UsageRecorded { usage } => {
            let Some(size) = context_size else {
                return Some(Err(Skipped::NoContextWindow));
            };
            // What the last request put into the context, plus what it produced
            // back: the next request will carry both, so this is the occupancy
            // after the step that reported it.
            SessionUpdate::UsageUpdate(UsageUpdate::new(
                usage.input_tokens + usage.output_tokens,
                size,
            ))
        }
        Event::TurnComplete { .. } => return Some(Err(Skipped::TurnComplete)),
        // The notification path handles this before reaching here, because the
        // turn it names is not the one the sink is streaming.
        Event::TurnDiscarded { .. } => return Some(Err(Skipped::TurnDiscarded)),
    };
    Some(Ok(update))
}

/// The update announcing a tool call whose arguments are still arriving.
///
/// Carries no `rawInput` and leaves `status` at ACP's default, which is
/// `Pending` — documented as a call whose input is still streaming, and omitted
/// from the wire because it is the default. So a client reads the absence as
/// "this call is not runnable yet" without a SrudAgent extension, and a client
/// that has never heard of this update still gets a well-formed `tool_call` it
/// can show.
fn tool_call_named(call_id: String, name: String) -> SessionUpdate {
    SessionUpdate::ToolCall(ToolCall::new(call_id, name.clone()).name(name))
}

/// The update announcing a tool call.
///
/// A `tool_call` rather than an update, for a client that has to handle this and
/// the replayed form of it as the same thing: a replayed call arrives whole, from
/// the log, with no earlier announcement to attach to. Live, [`tool_call_input`]
/// follows an announcement instead.
fn tool_call(call_id: String, name: String, arguments: serde_json::Value) -> SessionUpdate {
    SessionUpdate::ToolCall(
        ToolCall::new(call_id, name.clone())
            .name(name)
            .status(ToolCallStatus::InProgress)
            .raw_input(arguments),
    )
}

/// The update carrying a named call's arguments, live.
///
/// An update rather than a second `tool_call`, because the call already exists:
/// [`Event::ToolCallNamed`] announced it before the arguments were whole. Sending
/// the whole call twice would leave a client that treats the update as creating
/// something with two of them.
fn tool_call_input(call_id: String, name: String, arguments: serde_json::Value) -> SessionUpdate {
    SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
        call_id,
        ToolCallUpdateFields::new()
            .name(name)
            .status(ToolCallStatus::InProgress)
            .raw_input(arguments),
    ))
}

/// The arguments as a client should read them.
///
/// The log keeps them as the model produced them — a string, so a crash preserves
/// exactly what was asked for — and a client renders the object. Arguments that do
/// not parse are sent as null rather than omitted: the call happened either way,
/// and the tool is the one that has to say the arguments were wrong.
fn raw_arguments(arguments: &str) -> serde_json::Value {
    serde_json::from_str(arguments).unwrap_or(serde_json::Value::Null)
}

/// The update settling a tool call.
///
/// As [`tool_call`], and for the same reason.
fn tool_result(call_id: String, output: String, is_error: bool) -> SessionUpdate {
    let status = if is_error {
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
                    TextContent::new(output),
                )),
            )])
            .raw_output(serde_json::json!({ "is_error": is_error })),
    ))
}

/// A text chunk tagged with the message it belongs to.
///
/// The id is converted at the boundary rather than carried as core's own type:
/// core's `MessageId` is a log concept that happens to share the protocol's
/// spelling, and the two are free to diverge — the log's format is frozen, the
/// protocol's is not.
fn chunk(delta: String, message_id: Option<srud_core::session_event::MessageId>) -> ContentChunk {
    with_message_id(ContentBlock::Text(TextContent::new(delta)), message_id)
}

fn with_message_id(
    content: ContentBlock,
    message_id: Option<srud_core::session_event::MessageId>,
) -> ContentChunk {
    let chunk = ContentChunk::new(content);
    match message_id {
        Some(id) => chunk.message_id(srud_protocol::acp::MessageId::from(id.to_string())),
        None => chunk,
    }
}

/// Builds the `session/update` notification for a core event.
///
/// Returns `None` for events that have no wire representation (see
/// [`Skipped`]). `context_size` is the configured window of the session's
/// model, required for a usage report to be representable at all.
#[must_use]
pub fn to_notification(
    session_id: &SessionId,
    turn_id: &str,
    event: Event,
    context_size: Option<u64>,
) -> Option<Notification<SessionNotification>> {
    // The breakdown is read before the event is consumed by the mapping.
    let usage = match &event {
        Event::UsageRecorded { usage } => Some(srud_usage(usage)),
        _ => None,
    };
    // A discard names the turn it takes back, which is not the turn the sink is
    // streaming: it is emitted before the replacement starts and belongs to the
    // answer that is leaving. Read from the event rather than from `turn_id`,
    // which names whoever is speaking now.
    if let Event::TurnDiscarded { turn_id: discarded } = &event {
        return Some(discard_notification(session_id, &discarded.to_string()));
    }
    let update = match to_session_update(event, context_size)? {
        Ok(update) => update,
        Err(Skipped::TurnDiscarded) => return Some(discard_notification(session_id, turn_id)),
        Err(_) => return None,
    };
    let (update, outer_meta) = attach_turn_meta(update, turn_id, usage);
    let notification = SessionNotification::new(session_id.clone(), update).meta(outer_meta);
    Some(Notification {
        method: CLIENT_METHOD_NAMES.session_update.into(),
        params: Some(notification),
    })
}

/// The update that tells a client a turn's answer was taken back.
///
/// A `session_info_update` with no fields set: ACP offers no update for "forget
/// this", and the empty variant costs a client that does not know the marker
/// nothing — it has nothing to render and changes nothing. The turn to drop is
/// named in `_meta.srud.turnDiscarded`, which is also where the turn id of a
/// streaming update rides, so every other consumer of that field already parses
/// it.
fn discard_notification(
    session_id: &SessionId,
    turn_id: &str,
) -> Notification<SessionNotification> {
    let srud = SrudMeta {
        turn_discarded: Some(turn_id.to_string()),
        ..Default::default()
    };
    let meta = srud
        .to_meta()
        .expect("a payload naming a discarded turn is never empty");
    let update = SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new());
    let notification = SessionNotification::new(session_id.clone(), update).meta(Some(meta));
    Notification {
        method: CLIENT_METHOD_NAMES.session_update.into(),
        params: Some(notification),
    }
}

/// Attaches `_meta.srud` to an update.
///
/// The turn id goes on the **outer** `SessionNotification` rather than the inner
/// update variant, because several `SessionUpdate` variants do not carry a `meta`
/// field of their own, and a single placement keeps the reader uniform. The usage
/// breakdown rides with it, on the same placement, for the same reason.
fn attach_turn_meta(
    update: SessionUpdate,
    turn_id: &str,
    usage: Option<SrudUsage>,
) -> (SessionUpdate, Meta) {
    let srud = SrudMeta {
        turn_id: Some(turn_id.to_string()),
        usage,
        ..Default::default()
    };
    let meta = srud
        .to_meta()
        .expect("a payload with turnId is never empty");
    (update, meta)
}

/// Maps a core usage report onto its `_meta.srud.usage` breakdown.
fn srud_usage(usage: &TokenUsage) -> SrudUsage {
    SrudUsage {
        input_tokens: usage.input_tokens,
        output_tokens: usage.output_tokens,
        cached_input_tokens: usage.cached_input_tokens,
        cache_write_input_tokens: usage.cache_write_input_tokens,
        reasoning_output_tokens: usage.reasoning_output_tokens,
    }
}

/// The wire spelling of a turn-end reason.
///
/// The names a client already knows a turn end by: the variant names, not ACP's
/// stop reasons. The turns a `session/load` reports have no `StopReason` behind
/// them.
pub(crate) fn end_reason_wire(reason: TurnEndReason) -> &'static str {
    match reason {
        TurnEndReason::Completed => "completed",
        TurnEndReason::Interrupted => "interrupted",
        TurnEndReason::Blocked => "blocked",
        TurnEndReason::Error => "error",
    }
}

/// Builds the notification that replays one recorded item.
///
/// This is how `session/load` hands a client a conversation it was not there for.
/// The mapping is deliberately the same as the live one: a replayed call and a
/// live call go out as the same update through the same builders, so a client
/// cannot end up handling two shapes for one thing. How each turn ended is not part
/// of this stream; it rides on the `session/load` response.
///
/// Returns `None` for a record a client is not shown. The env-context block is
/// context the model reads — showing it would put the agent's own notes on screen
/// as something the user said — and a
/// [`Reasoning`](srud_core::types::ResponseItem::Reasoning) item **is** shown,
/// because a client renders thinking and the model is the one that never sees it
/// again.
#[must_use]
pub fn replay_notification(
    session_id: &SessionId,
    event: &SessionEvent,
    context_size: Option<u64>,
) -> Option<Notification<SessionNotification>> {
    // A recorded usage report goes out exactly as the live one did, so a client
    // sees one shape for it. Without a configured window it stays unshown, as
    // live it would have been.
    if let SessionEvent::UsageRecorded { turn_id, usage } = event {
        let size = context_size?;
        let update = SessionUpdate::UsageUpdate(UsageUpdate::new(
            usage.input_tokens + usage.output_tokens,
            size,
        ));
        let (update, outer_meta) =
            attach_turn_meta(update, &turn_id.to_string(), Some(srud_usage(usage)));
        let notification = SessionNotification::new(session_id.clone(), update).meta(outer_meta);
        return Some(Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(notification),
        });
    }

    let SessionEvent::Item {
        turn_id,
        message_id,
        item,
    } = event
    else {
        return None;
    };

    // Context the model reads, shown to nobody: putting it on screen would render
    // the agent's own notes as something the user said.
    if srud_core::context::is_item(item) {
        return None;
    }

    let update = match item {
        ResponseItem::Message { role, content } => match role {
            Role::User => {
                SessionUpdate::UserMessageChunk(chunk(content.clone(), message_id.clone()))
            }
            Role::Assistant => {
                SessionUpdate::AgentMessageChunk(chunk(content.clone(), message_id.clone()))
            }
            // Tool output travels as a `FunctionCallOutput`, which carries the
            // call id a `Message` would need to be attributed to one.
            Role::Tool => return None,
        },
        ResponseItem::Image { mime, data } => SessionUpdate::UserMessageChunk(with_message_id(
            ContentBlock::Image(ImageContent::new(data.clone(), mime.clone())),
            message_id.clone(),
        )),
        ResponseItem::Reasoning { content } => {
            SessionUpdate::AgentThoughtChunk(chunk(content.clone(), message_id.clone()))
        }
        ResponseItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => tool_call(call_id.clone(), name.clone(), raw_arguments(arguments)),
        ResponseItem::FunctionCallOutput {
            call_id,
            output,
            is_error,
        } => tool_result(call_id.clone(), output.clone(), *is_error),
    };

    let (update, outer_meta) = attach_turn_meta(update, &turn_id.to_string(), None);
    let notification = SessionNotification::new(session_id.clone(), update).meta(outer_meta);
    Some(Notification {
        method: CLIENT_METHOD_NAMES.session_update.into(),
        params: Some(notification),
    })
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

/// A prompt block that the declared capabilities do not accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedContent {
    /// An audio block, refused while `promptCapabilities.audio` is false.
    Audio,
    /// Any other block the agent does not read, such as a resource or a link.
    Other(&'static str),
}

/// The parts of a prompt the agent accepts.
#[derive(Debug, Default)]
pub struct PromptParts {
    /// The text blocks, joined in the order they arrived.
    pub text: String,
    /// The image blocks, in the order they arrived.
    pub images: Vec<srud_core::types::ImagePart>,
}

/// Splits a prompt into the text and images the agent accepts.
///
/// Returns an error naming the first block the agent cannot accept — dropping
/// multimodal input silently would make the model answer a prompt the user
/// never sent.
pub fn prompt_parts(blocks: &[ContentBlock]) -> Result<PromptParts, AcpError> {
    let mut parts = PromptParts::default();
    for block in blocks {
        match block {
            ContentBlock::Text(content) => {
                if !parts.text.is_empty() {
                    parts.text.push('\n');
                }
                parts.text.push_str(&content.text);
            }
            ContentBlock::Image(content) => parts.images.push(srud_core::types::ImagePart {
                mime: content.mime_type.clone(),
                data: content.data.clone(),
            }),
            block => {
                let kind = match block {
                    ContentBlock::Audio(_) => UnsupportedContent::Audio,
                    ContentBlock::Resource(_) => UnsupportedContent::Other("resource"),
                    ContentBlock::ResourceLink(_) => UnsupportedContent::Other("resource_link"),
                    _ => UnsupportedContent::Other("unknown"),
                };
                return Err(AcpError::new(
                    INVALID_PARAMS,
                    format!("this agent cannot accept {kind:?} prompts"),
                ));
            }
        }
    }
    Ok(parts)
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
        to_notification(&session_id(), "turn-1", event, Some(128_000))
            .expect("event maps to a notification")
    }

    #[test]
    fn turn_started_and_complete_are_not_updates() {
        use srud_core::types::TurnId;
        let turn_id = TurnId::new();
        assert_eq!(
            to_session_update(Event::TurnStarted { turn_id }, None),
            Some(Err(Skipped::TurnStarted))
        );
        assert_eq!(
            to_session_update(
                Event::TurnComplete {
                    turn_id,
                    reason: TurnEndReason::Completed,
                },
                None
            ),
            Some(Err(Skipped::TurnComplete))
        );
        assert!(
            to_notification(&session_id(), "t", Event::TurnStarted { turn_id }, None).is_none()
        );
    }

    #[test]
    fn a_usage_report_becomes_a_usage_update_with_the_breakdown_in_meta() {
        let usage = TokenUsage {
            input_tokens: 1_000,
            output_tokens: 250,
            cached_input_tokens: 800,
            cache_write_input_tokens: 0,
            reasoning_output_tokens: 40,
        };
        let n = notify(Event::UsageRecorded { usage });
        let SessionUpdate::UsageUpdate(update) = &n.params.as_ref().unwrap().update else {
            panic!("expected a usage_update");
        };
        assert_eq!(update.used, 1_250, "input plus output is the occupancy");
        assert_eq!(update.size, 128_000);
        let value = serde_json::to_value(&n).unwrap();
        assert_eq!(
            value["params"]["_meta"]["srud"]["usage"]["cachedInputTokens"],
            json!(800)
        );
        assert_eq!(
            value["params"]["_meta"]["srud"]["usage"]["reasoningOutputTokens"],
            json!(40)
        );
    }

    #[test]
    fn a_replayed_usage_record_becomes_the_same_usage_update() {
        let turn_id = srud_core::types::TurnId::new();
        let event = SessionEvent::UsageRecorded {
            turn_id,
            usage: TokenUsage {
                input_tokens: 500,
                output_tokens: 100,
                cached_input_tokens: 300,
                cache_write_input_tokens: 0,
                reasoning_output_tokens: 0,
            },
        };
        let n = replay_notification(&session_id(), &event, Some(128_000))
            .expect("a replayed usage record is shown");
        let SessionUpdate::UsageUpdate(update) = &n.params.as_ref().unwrap().update else {
            panic!("expected a usage_update");
        };
        assert_eq!(update.used, 600);
        assert_eq!(update.size, 128_000);
        let value = serde_json::to_value(&n).unwrap();
        assert_eq!(
            value["params"]["_meta"]["srud"]["usage"]["cachedInputTokens"],
            json!(300)
        );
        assert_eq!(
            value["params"]["_meta"]["srud"]["turnId"],
            json!(turn_id.to_string())
        );
        // Without a configured window the replayed record stays unshown, as
        // the live one would have.
        assert!(replay_notification(&session_id(), &event, None).is_none());
    }

    #[test]
    fn a_replayed_image_becomes_a_user_message_chunk() {
        let event = SessionEvent::item(
            srud_core::types::TurnId::new(),
            Some(srud_core::session_event::MessageId::new()),
            ResponseItem::Image {
                mime: "image/png".into(),
                data: "AAAA".into(),
            },
        );
        let n = replay_notification(&session_id(), &event, Some(128_000))
            .expect("a replayed image is shown");
        let SessionUpdate::UserMessageChunk(chunk) = &n.params.as_ref().unwrap().update else {
            panic!("expected a user_message_chunk");
        };
        let ContentBlock::Image(image) = &chunk.content else {
            panic!("expected an image block");
        };
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.data, "AAAA");
    }

    #[test]
    fn a_usage_report_without_a_configured_window_is_not_shown() {
        assert_eq!(
            to_session_update(
                Event::UsageRecorded {
                    usage: TokenUsage::default(),
                },
                None
            ),
            Some(Err(Skipped::NoContextWindow))
        );
        assert!(to_notification(
            &session_id(),
            "t",
            Event::UsageRecorded {
                usage: TokenUsage::default(),
            },
            None
        )
        .is_none());
    }

    #[test]
    fn message_deltas_map_to_chunk_variants() {
        let n = notify(Event::AgentMessageDelta {
            message_id: None,
            delta: "hello".into(),
        });
        assert!(matches!(
            n.params.as_ref().unwrap().update,
            SessionUpdate::AgentMessageChunk(_)
        ));
        let n = notify(Event::AgentThoughtDelta {
            message_id: None,
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
    fn a_named_tool_call_carries_no_input_and_leaves_the_status_at_pending() {
        // ACP's default status is `Pending`, documented as a call whose input is
        // still streaming, and it is skipped on the wire because it is the default.
        // So a client reads the absence as "not runnable yet" with no SrudAgent
        // extension involved.
        let n = notify(Event::ToolCallNamed {
            call_id: "tc_1".into(),
            name: "write".into(),
        });
        let value = serde_json::to_value(n.params.as_ref().unwrap()).unwrap();
        let SessionUpdate::ToolCall(call) = n.params.unwrap().update else {
            panic!("expected a tool_call");
        };

        assert_eq!(call.name.as_deref(), Some("write"));
        assert_eq!(call.raw_input, None, "no arguments have arrived yet");
        assert_eq!(call.status, ToolCallStatus::Pending);
        assert!(
            value.get("status").is_none(),
            "and so the field is absent rather than spelled out: {value}"
        );
        assert_eq!(value.get("rawInput"), None, "{value}");
    }

    #[test]
    fn tool_call_begin_carries_input_and_in_progress_status() {
        let n = notify(Event::ToolCallBegin {
            call_id: "c1".into(),
            name: "echo".into(),
            arguments: json!({ "text": "hi" }),
        });
        let SessionUpdate::ToolCallUpdate(update) = &n.params.as_ref().unwrap().update else {
            panic!("an update: the call was announced before it had arguments");
        };
        assert_eq!(update.fields.name.as_deref(), Some("echo"));
        assert_eq!(update.fields.status, Some(ToolCallStatus::InProgress));
        assert_eq!(update.fields.raw_input, Some(json!({ "text": "hi" })));
    }

    #[test]
    fn a_replayed_call_arrives_whole_where_a_live_one_arrives_in_two() {
        // A replayed call comes from the log with nothing before it, so it is one
        // `tool_call`; live it is announced and then filled in. Both are well
        // formed, and the frontend handles each on its own terms.
        assert!(matches!(
            tool_call("c1".into(), "echo".into(), json!({ "text": "hi" })),
            SessionUpdate::ToolCall(_)
        ));
        let live = notify(Event::ToolCallBegin {
            call_id: "c1".into(),
            name: "echo".into(),
            arguments: json!({ "text": "hi" }),
        });
        assert!(matches!(
            live.params.unwrap().update,
            SessionUpdate::ToolCallUpdate(_)
        ));
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
        let n = notify(Event::AgentMessageDelta {
            message_id: None,
            delta: "x".into(),
        });
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

    #[test]
    fn prompt_parts_accepts_text_and_images_and_rejects_audio() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("hello")),
            ContentBlock::Image(srud_protocol::acp::ImageContent::new("d", "image/png")),
        ];
        let parts = prompt_parts(&blocks).unwrap();
        assert_eq!(parts.text, "hello");
        assert_eq!(
            parts.images,
            vec![srud_core::types::ImagePart {
                mime: "image/png".into(),
                data: "d".into(),
            }]
        );

        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Audio(srud_protocol::acp::AudioContent::new("d", "audio/wav")),
        ];
        let err = prompt_parts(&blocks).unwrap_err();
        assert_eq!(i32::from(err.code), INVALID_PARAMS);
        assert!(err.message.contains("Audio"));
    }
}
