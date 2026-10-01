//! Turning recorded history into a model request.
//!
//! Read-only: this inspects [`SessionState`] and produces a [`ModelRequest`].

use crate::client::{ModelRequest, ModelRequestItem};
use crate::session::SessionState;
use crate::types::{ResponseItem, Role};

/// The default system instruction.
pub const DEFAULT_INSTRUCTIONS: &str = "You are SrudAgent, an AI Agent. \
Work from the working directory you are pointed at. Prefer reading before \
writing, keep changes focused, and explain what you did in plain language.";

/// Builds a model request from the current state.
///
/// `tools` is passed in rather than read from the state because the tool set is
/// configuration, not history.
#[must_use]
pub fn build_prompt(
    state: &SessionState,
    tools: Vec<crate::tools::ToolDefinition>,
    instructions: Option<&str>,
) -> ModelRequest {
    let items = state.history().iter().filter_map(to_request_item).collect();

    ModelRequest {
        items,
        tools,
        instructions: Some(instructions.unwrap_or(DEFAULT_INSTRUCTIONS).to_owned()),
    }
}

/// Projects one recorded item into a request item.
///
/// Returns `None` for recorded items that are not sent back to the model.
fn to_request_item(item: &ResponseItem) -> Option<ModelRequestItem> {
    match item {
        ResponseItem::Message { role, content } => Some(ModelRequestItem::Message {
            role: *role,
            content: content.clone(),
        }),
        ResponseItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => Some(ModelRequestItem::FunctionCall {
            call_id: call_id.clone(),
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        ResponseItem::FunctionCallOutput { call_id, output } => {
            Some(ModelRequestItem::FunctionCallOutput {
                call_id: call_id.clone(),
                output: output.clone(),
            })
        }
        ResponseItem::Reasoning { .. } => None,
    }
}

/// Whether a recorded item is visible to the model.
///
/// Reasoning is recorded but not sent back.
#[must_use]
pub fn is_model_visible(item: &ResponseItem) -> bool {
    !matches!(item, ResponseItem::Reasoning { .. })
}

/// Builds the item recorded when the user submits input.
#[must_use]
pub fn user_item(text: &str) -> ResponseItem {
    ResponseItem::Message {
        role: Role::User,
        content: text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_maps_into_request_items_in_order() {
        let mut state = SessionState::new();
        state.push(user_item("hello"));
        state.push(ResponseItem::assistant("hi"));
        state.push(ResponseItem::FunctionCall {
            call_id: "c1".into(),
            name: "echo".into(),
            arguments: "{}".into(),
        });
        state.push(ResponseItem::FunctionCallOutput {
            call_id: "c1".into(),
            output: "hi".into(),
        });

        let request = build_prompt(&state, Vec::new(), None);
        assert_eq!(request.items.len(), 4);
        assert!(matches!(
            request.items[0],
            ModelRequestItem::Message {
                role: Role::User,
                ..
            }
        ));
        assert!(matches!(
            request.items[2],
            ModelRequestItem::FunctionCall { .. }
        ));
        assert!(matches!(
            request.items[3],
            ModelRequestItem::FunctionCallOutput { .. }
        ));
    }

    #[test]
    fn reasoning_is_recorded_but_not_sent_back() {
        let mut state = SessionState::new();
        state.push(ResponseItem::Reasoning {
            content: "internal".into(),
        });
        state.push(user_item("hi"));

        let request = build_prompt(&state, Vec::new(), None);
        assert_eq!(
            request.items.len(),
            1,
            "reasoning must not appear in the request"
        );
        assert_eq!(state.len(), 2, "but it stays in the record");
    }

    #[test]
    fn instructions_default_when_not_given() {
        let state = SessionState::new();
        let request = build_prompt(&state, Vec::new(), None);
        assert_eq!(request.instructions.as_deref(), Some(DEFAULT_INSTRUCTIONS));
    }

    #[test]
    fn instructions_can_be_overridden() {
        let state = SessionState::new();
        let request = build_prompt(&state, Vec::new(), Some("be terse"));
        assert_eq!(request.instructions.as_deref(), Some("be terse"));
    }

    #[test]
    fn model_visibility_excludes_only_reasoning() {
        assert!(is_model_visible(&user_item("hi")));
        assert!(!is_model_visible(&ResponseItem::Reasoning {
            content: "x".into()
        }));
    }
}
