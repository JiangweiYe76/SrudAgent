//! Turning recorded history into a model request.
//!
//! Read-only: this inspects [`SessionState`] and produces a [`ModelRequest`].
//!
//! The system instruction is assembled from [`Section`] blocks rather than
//! written as one string, so a block can be added, dropped, or reordered
//! without touching the content of its neighbours.

use crate::client::{ModelRequest, ModelRequestItem};
use crate::session::SessionState;
use crate::types::{ResponseItem, Role};

/// One titled block of the system instruction.
///
/// The heading belongs to the block, not to its body: [`Section::body`] holds
/// content only and [`render_instructions`] writes the heading. A title is
/// therefore spelled once, where the block is declared, and cannot drift out
/// of sync with the text under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The heading, rendered as a markdown level-one title.
    pub title: &'static str,
    /// The content under the heading.
    pub body: String,
}

impl Section {
    /// Declares a block: a heading and the content beneath it.
    #[must_use]
    pub fn new(title: &'static str, body: impl Into<String>) -> Self {
        Self {
            title,
            body: body.into(),
        }
    }
}

/// The role block: who the model is.
const ROLE_BODY: &str = "You are Srud, an AI Agent.";

/// The working-style block: how the model is expected to go about the work.
///
/// A list, one rule per item, so no rule reads as a qualification of the one
/// before it.
const WORKING_STYLE_BODY: &str = "\
- Look before acting: gather what you need first, and never change anything you have not checked.
- Do only what the task asks; leave alone everything it does not need you to touch.
- Do not guess: if a fact is not in front of you, go get it, or say you could not find it.
- Answer in the language the user writes in.";

/// The blocks the system instruction is built from when the caller supplies
/// none of its own.
///
/// Returned owned so a caller can extend the list: blocks are ordered, and
/// adding one means choosing where it goes, not just whether it is present.
#[must_use]
pub fn default_sections() -> Vec<Section> {
    vec![
        Section::new("Role", ROLE_BODY),
        Section::new("Working style", WORKING_STYLE_BODY),
    ]
}

/// Renders blocks into the system instruction.
///
/// Every block becomes `# {title}` followed by its content. A block with
/// nothing to say is dropped whole, so a heading never stands on its own: a
/// block whose text is derived at runtime can come back empty. Order is
/// preserved because where two blocks disagree, the later one is the one the
/// model is expected to follow.
#[must_use]
pub fn render_instructions(sections: &[Section]) -> String {
    sections
        .iter()
        .filter(|section| !section.body.trim().is_empty())
        .map(|section| format!("# {}\n\n{}", section.title, section.body.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Builds a model request from the current state.
///
/// `tools` is passed in rather than read from the state because the tool set is
/// configuration, not history. `sections` is `None` to build from
/// [`default_sections`]; a caller with blocks of its own passes the whole list,
/// since the rendering depends on their order.
#[must_use]
pub fn build_prompt(
    state: &SessionState,
    tools: Vec<crate::tools::ToolDefinition>,
    sections: Option<&[Section]>,
) -> ModelRequest {
    let items = state.history().iter().filter_map(to_request_item).collect();
    let instructions = match sections {
        Some(sections) => render_instructions(sections),
        None => render_instructions(&default_sections()),
    };

    ModelRequest {
        items,
        tools,
        instructions: Some(instructions),
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
    fn instructions_default_to_the_rendered_default_blocks() {
        let state = SessionState::new();
        let request = build_prompt(&state, Vec::new(), None);
        assert_eq!(
            request.instructions.as_deref(),
            Some(render_instructions(&default_sections()).as_str())
        );
    }

    #[test]
    fn caller_blocks_replace_the_defaults() {
        let state = SessionState::new();
        let sections = vec![Section::new("Style", "be terse")];
        let request = build_prompt(&state, Vec::new(), Some(&sections));
        assert_eq!(
            request.instructions.as_deref(),
            Some("# Style\n\nbe terse"),
            "a caller's blocks are rendered as given"
        );
    }

    #[test]
    fn a_block_is_rendered_as_a_heading_over_its_content() {
        let rendered = render_instructions(&[Section::new("Role", "You are Srud.")]);
        assert_eq!(rendered, "# Role\n\nYou are Srud.");
    }

    #[test]
    fn blocks_keep_their_order_and_are_separated() {
        let rendered =
            render_instructions(&[Section::new("Role", "who"), Section::new("Style", "how")]);
        assert_eq!(rendered, "# Role\n\nwho\n\n# Style\n\nhow");
    }

    #[test]
    fn a_block_with_nothing_to_say_is_dropped_whole() {
        let rendered = render_instructions(&[
            Section::new("Role", "who"),
            Section::new("Project", "   \n  "),
        ]);
        assert_eq!(
            rendered, "# Role\n\nwho",
            "an empty block must not leave its heading behind"
        );
    }

    #[test]
    fn leading_and_trailing_space_in_a_body_is_trimmed() {
        let rendered = render_instructions(&[Section::new("Role", "\n  You are Srud. \n")]);
        assert_eq!(rendered, "# Role\n\nYou are Srud.");
    }

    #[test]
    fn the_default_blocks_are_role_then_working_style() {
        let sections = default_sections();
        let titles: Vec<_> = sections.iter().map(|section| section.title).collect();
        assert_eq!(titles, vec!["Role", "Working style"]);
        assert!(
            sections[0].body.starts_with("You are Srud,"),
            "the role block names the agent: {:?}",
            sections[0].body
        );
        assert!(
            !sections[0].body.contains("Look before acting"),
            "how to work belongs to the working-style block, not the role one"
        );
    }

    #[test]
    fn the_working_style_block_is_a_list_of_rules() {
        let body = &default_sections()[1].body;
        let items: Vec<_> = body.lines().collect();
        assert_eq!(items.len(), 4, "one rule per item: {body:?}");
        assert!(
            items.iter().all(|item| item.starts_with("- ")),
            "every rule is a list item: {body:?}"
        );
    }

    #[test]
    fn model_visibility_excludes_only_reasoning() {
        assert!(is_model_visible(&user_item("hi")));
        assert!(!is_model_visible(&ResponseItem::Reasoning {
            content: "x".into()
        }));
    }
}
