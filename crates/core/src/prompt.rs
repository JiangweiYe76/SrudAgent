//! Turning recorded history into a model request.
//!
//! Read-only: this inspects [`SessionState`] and produces a [`ModelRequest`].
//!
//! The system instruction is assembled from [`Section`] blocks rather than
//! written as one string, so a block can be added, dropped, or reordered
//! without touching the content of its neighbours. A session renders it once and
//! keeps it: what changes afterwards is described as [`ContextChange`]s rather
//! than written into a prefix the model has already been sent.

use std::collections::BTreeMap;

use chrono::{DateTime, Local};

use crate::client::{ModelRequest, ModelRequestItem};
use crate::session::SessionState;
use crate::types::{ImagePart, ResponseItem, Role};

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

/// The context block: how to read the tagged messages that describe the session
/// rather than the conversation.
const CONTEXT_BODY: &str = "\
- `<env-context>` opens the first turn: the directory the session works in, and the local date and time.
- `<env-context-update>` appears before a user message when a skill or MCP server was added or removed, or the date has moved on.
- Both describe the environment; read them as facts, not as requests.";

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
        Section::new("Context", CONTEXT_BODY),
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

/// How stale the date in the instruction may get before a new one is due.
///
/// An hour is long enough that a date does not turn stale every turn, and short
/// enough that "today" still names the day the model was told it is.
pub const TIME_UPDATE_AFTER_SECS: i64 = 60 * 60;

/// One difference between the instruction the model was given and the one the
/// session would render now.
///
/// Only what changes what the model can reach is reported: a skill or a server
/// appearing or disappearing, and a date that has gone stale. A reworded
/// description is left out — it leaves the model just as capable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextChange {
    /// The instruction's date is more than [`TIME_UPDATE_AFTER_SECS`] old.
    Time { now: DateTime<Local> },
    /// A skill the instruction did not advertise.
    SkillAdded { name: String, description: String },
    /// A skill the instruction advertised that is gone.
    SkillRemoved(String),
    /// A server the instruction did not describe.
    ServerAdded { name: String, instructions: String },
    /// A server the instruction described that is gone.
    ServerRemoved(String),
}

/// What changed about a session's surroundings since its instruction settled.
///
/// `settled` is the set of blocks the model was given, `current` the blocks the
/// session would render now, and `last_time` the date the instruction carried.
/// Empty when nothing has changed.
#[must_use]
pub fn context_changes(
    settled: &[Section],
    current: &[Section],
    last_time: Option<DateTime<Local>>,
    now: DateTime<Local>,
) -> Vec<ContextChange> {
    let mut changes = Vec::new();

    if let Some(last) = last_time {
        if now.signed_duration_since(last).num_seconds() > TIME_UPDATE_AFTER_SECS {
            changes.push(ContextChange::Time { now });
        }
    }

    let before = skills_in(settled);
    let after = skills_in(current);
    for (name, description) in &after {
        if !before.contains_key(name) {
            changes.push(ContextChange::SkillAdded {
                name: name.clone(),
                description: description.clone(),
            });
        }
    }
    for name in before.keys() {
        if !after.contains_key(name) {
            changes.push(ContextChange::SkillRemoved(name.clone()));
        }
    }

    let before = servers_in(settled);
    let after = servers_in(current);
    for (name, instructions) in &after {
        if !before.contains_key(name) {
            changes.push(ContextChange::ServerAdded {
                name: name.clone(),
                instructions: instructions.clone(),
            });
        }
    }
    for name in before.keys() {
        if !after.contains_key(name) {
            changes.push(ContextChange::ServerRemoved(name.clone()));
        }
    }

    changes
}

/// The skills a set of blocks advertises, by name.
fn skills_in(sections: &[Section]) -> BTreeMap<String, String> {
    block(sections, "Skills")
        .into_iter()
        .flat_map(|body| body.lines())
        .filter_map(|line| line.strip_prefix("- "))
        .filter_map(|entry| entry.split_once(':'))
        .map(|(name, description)| (name.trim().to_owned(), description.trim().to_owned()))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

/// The servers a set of blocks describes, by name.
fn servers_in(sections: &[Section]) -> BTreeMap<String, String> {
    let Some(body) = block(sections, "MCP servers") else {
        return BTreeMap::new();
    };
    body.split("\n## ")
        .map(|part| part.trim_start_matches("## "))
        .filter_map(|part| part.split_once('\n'))
        .map(|(name, instructions)| (name.trim().to_owned(), instructions.trim().to_owned()))
        .filter(|(name, _)| !name.is_empty())
        .collect()
}

/// The body of the block with this title, when it has one.
fn block<'a>(sections: &'a [Section], title: &str) -> Option<&'a str> {
    sections
        .iter()
        .find(|section| section.title == title)
        .map(|section| section.body.as_str())
}

/// Builds a model request from the current state.
///
/// `tools` is passed in rather than read from the state because the tool set is
/// configuration, not history. `instruction` is the settled system instruction:
/// whether this session is starting with one or re-sending the one it settled on,
/// the caller decides, since a prefix that changes costs the provider's cache.
#[must_use]
pub fn build_prompt(
    state: &SessionState,
    tools: Vec<crate::tools::ToolDefinition>,
    instruction: &str,
) -> ModelRequest {
    let items = state.history().iter().filter_map(to_request_item).collect();

    ModelRequest {
        items,
        tools,
        instructions: Some(instruction.to_owned()),
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
        ResponseItem::Image { mime, data } => Some(ModelRequestItem::Image {
            mime: mime.clone(),
            data: data.clone(),
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
        ResponseItem::FunctionCallOutput {
            call_id,
            output,
            // Dropped at the boundary: the provider's shape has nowhere for it,
            // and the model reads the failure from the text.
            ..
        } => Some(ModelRequestItem::FunctionCallOutput {
            call_id: call_id.clone(),
            output: output.clone(),
        }),
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

/// Builds the item recorded for an image submitted with user input.
#[must_use]
pub fn image_item(image: &ImagePart) -> ResponseItem {
    ResponseItem::Image {
        mime: image.mime.clone(),
        data: image.data.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instruction a session starts with, rendered once.
    fn instruction() -> String {
        render_instructions(&default_sections())
    }

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
            is_error: false,
        });

        let request = build_prompt(&state, Vec::new(), &instruction());
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
    fn images_project_after_the_text_they_were_sent_with() {
        let mut state = SessionState::new();
        state.push(user_item("what is this?"));
        state.push(image_item(&ImagePart {
            mime: "image/png".into(),
            data: "AAAA".into(),
        }));

        let request = build_prompt(&state, Vec::new(), &instruction());
        assert_eq!(request.items.len(), 2);
        assert!(matches!(
            &request.items[1],
            ModelRequestItem::Image { mime, data } if mime == "image/png" && data == "AAAA"
        ));
    }

    #[test]
    fn reasoning_is_recorded_but_not_sent_back() {
        let mut state = SessionState::new();
        state.push(ResponseItem::Reasoning {
            content: "internal".into(),
        });
        state.push(user_item("hi"));

        let request = build_prompt(&state, Vec::new(), &instruction());
        assert_eq!(
            request.items.len(),
            1,
            "reasoning must not appear in the request"
        );
        assert_eq!(state.len(), 2, "but it stays in the record");
    }

    #[test]
    fn the_instruction_is_the_one_the_caller_settled_on() {
        let state = SessionState::new();
        let settled = instruction();
        let request = build_prompt(&state, Vec::new(), &settled);
        assert_eq!(request.instructions.as_deref(), Some(settled.as_str()));
    }

    #[test]
    fn a_callers_blocks_render_into_the_instruction() {
        let state = SessionState::new();
        let sections = vec![Section::new("Style", "be terse")];
        let request = build_prompt(&state, Vec::new(), &render_instructions(&sections));
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
    fn the_default_blocks_are_role_working_style_and_context() {
        let sections = default_sections();
        let titles: Vec<_> = sections.iter().map(|section| section.title).collect();
        assert_eq!(titles, vec!["Role", "Working style", "Context"]);
        assert!(
            sections[0].body.starts_with("You are Srud,"),
            "the role block names the agent: {:?}",
            sections[0].body
        );
        assert!(
            !sections[0].body.contains("Look before acting"),
            "how to work belongs to the working-style block, not the role one"
        );
        assert!(
            sections[2].body.contains("<env-context>")
                && sections[2].body.contains("<env-context-update>"),
            "the context block names both tags the runtime writes: {:?}",
            sections[2].body
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

    #[test]
    fn an_unmoved_instruction_has_no_changes() {
        let now = now();
        let changes = context_changes(&default_sections(), &default_sections(), Some(now), now);
        assert!(changes.is_empty(), "nothing moved: {changes:?}");
    }

    #[test]
    fn a_skill_appearing_or_vanishing_is_a_change() {
        let settled = [
            Section::new("Role", "who"),
            Section::new("Skills", "- alpha: first\n- beta: second"),
        ];
        let current = [
            Section::new("Role", "who"),
            Section::new("Skills", "- beta: second\n- gamma: third"),
        ];

        let changes = context_changes(&settled, &current, None, now());

        assert_eq!(
            changes,
            vec![
                ContextChange::SkillAdded {
                    name: "gamma".into(),
                    description: "third".into(),
                },
                ContextChange::SkillRemoved("alpha".into()),
            ],
            "only the names that came or went are reported"
        );
    }

    #[test]
    fn a_reworded_description_is_not_a_change() {
        let settled = [Section::new("Skills", "- alpha: old words")];
        let current = [Section::new("Skills", "- alpha: new words")];

        assert!(
            context_changes(&settled, &current, None, now()).is_empty(),
            "the model is not interrupted for a reworded description"
        );
    }

    #[test]
    fn a_server_appearing_is_a_change() {
        let settled = [Section::new("MCP servers", "## docs\n\nread docs")];
        let current = [Section::new(
            "MCP servers",
            "## docs\n\nread docs\n\n## remote\n\ncall remote",
        )];

        assert_eq!(
            context_changes(&settled, &current, None, now()),
            vec![ContextChange::ServerAdded {
                name: "remote".into(),
                instructions: "call remote".into(),
            }]
        );
    }

    #[test]
    fn the_date_is_refreshed_only_after_an_hour() {
        let settled = default_sections();
        let current = default_sections();
        let now = now();

        let recent = now - chrono::Duration::minutes(59);
        assert!(
            context_changes(&settled, &current, Some(recent), now).is_empty(),
            "an hour has not passed"
        );

        let stale = now - chrono::Duration::minutes(61);
        assert_eq!(
            context_changes(&settled, &current, Some(stale), now),
            vec![ContextChange::Time { now }],
            "a stale date is worth telling the model about"
        );
    }

    /// A fixed clock, so a threshold test does not depend on when it runs.
    fn now() -> DateTime<Local> {
        use chrono::TimeZone;
        Local
            .with_ymd_and_hms(2026, 11, 15, 10, 30, 0)
            .single()
            .expect("unambiguous local time")
    }
}
