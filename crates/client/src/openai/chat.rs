//! OpenAI Chat Completions adapter.
//!
//! Translates a `/chat/completions` stream into the core's [`ModelEvent`]
//! vocabulary. The stream is flatter than the Responses API's: every chunk
//! carries a list of choices whose deltas hold whatever arrived next.
//!
//! # Assembling the fragments
//!
//! The SDK stops at deserializing one SSE event into one chunk. It does not
//! reassemble a tool call from the fragments that carry it, and cannot: this
//! protocol sends an `index`-keyed fragment per chunk with no marker for "this
//! call is complete", so how the pieces combine is a decision only the caller has
//! the context to make. The Responses API needs no such step — its events carry
//! their own types and a `finalized` signal — which is why that adapter is a
//! mapping and this one has a [`CallAssembly`].
//!
//! So the policy here is ours to get right, and it is provider-dependent:
//! [`CallAssembly::absorb`] documents the shape that broke it.

use std::collections::BTreeMap;

use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCall, ChatCompletionMessageToolCallChunk,
    ChatCompletionMessageToolCalls, ChatCompletionRequestAssistantMessage,
    ChatCompletionRequestAssistantMessageContent, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestSystemMessageContent,
    ChatCompletionRequestToolMessage, ChatCompletionRequestToolMessageContent,
    ChatCompletionRequestUserMessage, ChatCompletionRequestUserMessageContent, ChatCompletionTool,
    ChatCompletionTools, CreateChatCompletionRequest, CreateChatCompletionRequestArgs,
    FunctionCall, FunctionObject,
};
use async_openai::Client;
use futures::StreamExt;
use srud_core::client::{
    ModelClient, ModelError, ModelEvent, ModelRequest, ModelRequestItem, ModelStream,
};
use srud_core::tools::ToolDefinition;
use srud_core::types::Role;
use tokio_util::sync::CancellationToken;

use super::{connect, ClientConfig};

/// A client for the Chat Completions endpoint.
///
/// Holds the provider SDK's client and the model id, and implements the core's
/// [`ModelClient`] by streaming a completion and translating each chunk.
pub struct ChatClient {
    client: Client<OpenAIConfig>,
    model: String,
}

impl ChatClient {
    /// Creates a client from explicit settings.
    #[must_use]
    pub fn new(settings: ClientConfig) -> Self {
        let (client, model) = connect(settings);
        Self { client, model }
    }

    /// Creates a client from the environment.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::Config`] if [`ClientConfig::from_env`] fails.
    pub fn from_env() -> Result<Self, ModelError> {
        Ok(Self::new(ClientConfig::from_env()?))
    }

    /// Returns the model this client requests.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }
}

#[async_trait::async_trait]
impl ModelClient for ChatClient {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let sdk_request = to_request(&self.model, &request);

        let mut source = self
            .client
            .chat()
            .create_stream_byot::<_, StreamChunk>(sdk_request)
            .await
            .map_err(|error| ModelError::Rejected(error.to_string()))?;

        // This endpoint has no per-call completion event: fragments accumulate
        // by index and a call is only whole once the stream ends, which is also
        // the only place a terminator can come from.
        let stream = async_stream::stream! {
            let mut calls = CallAssembly::default();
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                let Some(next) = source.next().await else {
                    break;
                };
                let chunk = match next {
                    Ok(chunk) => chunk,
                    Err(error) => {
                        yield Err(ModelError::Transport(error.to_string()));
                        return;
                    }
                };
                for event in fold_chunk(chunk, &mut calls) {
                    yield Ok(event);
                }
            }

            for call in calls.take() {
                yield Ok(call);
            }
            yield Ok(ModelEvent::Done);
        };

        Ok(Box::pin(stream))
    }
}

/// One streamed chunk of a chat-completions response.
///
/// The SDK's own chunk type has nowhere to put `reasoning_content` — a
/// non-standard field that DeepSeek, Kimi, Qwen and others stream their
/// thinking in. Serde drops unknown fields, so streaming parses into this type
/// instead, which declares the field.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct StreamChunk {
    /// One entry per sampled alternative; only the first is requested here.
    ///
    /// Empty on the trailing usage-only chunk some providers send.
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
}

/// One alternative of a streamed chunk.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct ChunkChoice {
    /// What arrived for this alternative in this chunk.
    #[serde(default)]
    pub delta: ChunkDelta,
}

/// The increment a chunk carries.
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
pub struct ChunkDelta {
    /// Assistant-visible text.
    pub content: Option<String>,
    /// Reasoning text, on providers that expose it.
    pub reasoning_content: Option<String>,
    /// Tool-call fragments, keyed by index across chunks.
    pub tool_calls: Option<Vec<ChatCompletionMessageToolCallChunk>>,
}

/// Folds one chunk into whatever it carries, returning the events to forward.
fn fold_chunk(chunk: StreamChunk, calls: &mut CallAssembly) -> Vec<ModelEvent> {
    let mut events = Vec::new();
    for choice in chunk.choices {
        // Reasoning is emitted ahead of the text it precedes.
        if let Some(reasoning) = choice.delta.reasoning_content {
            if !reasoning.is_empty() {
                events.push(ModelEvent::ThoughtDelta { delta: reasoning });
            }
        }
        if let Some(content) = choice.delta.content {
            if !content.is_empty() {
                events.push(ModelEvent::TextDelta { delta: content });
            }
        }
        for fragment in choice.delta.tool_calls.into_iter().flatten() {
            // Ahead of the runnable call, so a call taking a large payload is
            // visible while the model is still writing it.
            if let Some(named) = calls.named(fragment.clone()) {
                events.push(named);
            }
        }
    }
    events
}

/// A tool call being assembled from stream fragments.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// Collects tool-call fragments.
///
/// A call arrives across several chunks keyed by index, and no single chunk is
/// the call. The id and name are headers rather than payload: some providers send
/// them on the first chunk only and leave them out or blank afterwards, others
/// repeat them in full every time. [`CallAssembly::absorb`] is where those two
/// shapes are reconciled, which is why the policy lives on the type rather than in
/// the fold.
#[derive(Debug, Default)]
struct CallAssembly {
    calls: BTreeMap<u32, PendingCall>,
}

impl CallAssembly {
    /// Folds one fragment into the call it belongs to.
    ///
    /// The id and name are **not** overwritten by an empty fragment. DashScope's
    /// OpenAI-compatible endpoint sends both on the first chunk of a call and `""`
    /// on every chunk after it, so taking the later value left every call with no
    /// name at all: the runtime dispatched it and the tool answered that there is
    /// no tool by that name — which reads like a wiring fault rather than as the
    /// protocol quirk it is. The model then saw its own call come back nameless and
    /// retried, over and over.
    ///
    /// Blank is not the same as absent, which is what makes this necessary rather
    /// than defensive: `Option::is_none` does not cover `Some("")`.
    ///
    /// A non-empty value still replaces an earlier one, because a provider that
    /// corrects itself mid-stream should be believed.
    fn absorb(&mut self, fragment: ChatCompletionMessageToolCallChunk) {
        let call = self.calls.entry(fragment.index).or_default();
        if let Some(id) = fragment.id.filter(|id| !id.is_empty()) {
            call.call_id = id;
        }
        if let Some(function) = fragment.function {
            if let Some(name) = function.name.filter(|name| !name.is_empty()) {
                call.name = name;
            }
            if let Some(arguments) = function.arguments {
                call.arguments.push_str(&arguments);
            }
        }
    }

    /// The call this fragment named, the first time it names one.
    ///
    /// The name arrives in a header and the arguments in pieces after it, so the
    /// moment a call becomes identifiable is well before it becomes runnable. A
    /// provider that repeats the name on every fragment names it once here.
    fn named(&mut self, fragment: ChatCompletionMessageToolCallChunk) -> Option<ModelEvent> {
        let index = fragment.index;
        let before = self
            .calls
            .get(&index)
            .is_none_or(|call| call.name.is_empty());
        self.absorb(fragment);
        let call = self.calls.get(&index)?;
        if !before && !call.name.is_empty() {
            return None;
        }
        (!call.name.is_empty()).then(|| ModelEvent::ToolCallNamed {
            call_id: call.call_id.clone(),
            name: call.name.clone(),
        })
    }

    /// Drains the assembled calls in index order.
    ///
    /// A call with no name is dropped rather than forwarded. Nothing can act on it,
    /// and forwarding it turns this adapter's parsing problem into a runtime error
    /// that reads as the model's: the registry answers "there is no tool by that
    /// name", and the model — which just asked correctly — retries the same call
    /// until the turn ends. Dropping it here says nothing either, which is better
    /// than saying the wrong thing.
    ///
    /// The id is not checked. A nameless call has no use for one, and a provider
    /// that names a call but withholds its id is within what the wire allows.
    fn take(&mut self) -> Vec<ModelEvent> {
        std::mem::take(&mut self.calls)
            .into_values()
            .filter(|call| !call.name.is_empty())
            .map(|call| ModelEvent::ToolCall {
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
            })
            .collect()
    }
}

/// Builds the SDK request from a core request.
#[must_use]
pub fn to_request(model: &str, request: &ModelRequest) -> CreateChatCompletionRequest {
    let mut messages = Vec::with_capacity(request.items.len() + 1);
    if let Some(instructions) = &request.instructions {
        messages.push(ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(instructions.clone()),
                name: None,
            },
        ));
    }
    messages.extend(request.items.iter().map(to_message));

    CreateChatCompletionRequestArgs::default()
        .model(model)
        .messages(messages)
        .tools(request.tools.iter().map(to_tool).collect::<Vec<_>>())
        // The BYOT streaming call sends this request as built, so the flag the
        // SDK would otherwise set for a stream has to be set here.
        .stream(true)
        .build()
        .unwrap_or_default()
}

/// Maps one recorded request item onto an API message.
///
/// Each recorded call becomes its own assistant message, which the API accepts
/// because the matching tool message follows immediately.
fn to_message(item: &ModelRequestItem) -> ChatCompletionRequestMessage {
    match item {
        ModelRequestItem::Message { role, content } => match role {
            Role::Assistant => {
                ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
                    content: Some(ChatCompletionRequestAssistantMessageContent::Text(
                        content.clone(),
                    )),
                    ..Default::default()
                })
            }
            // A `Tool`-role message carries no call id, so it can only be a
            // user message; tool results travel as `FunctionCallOutput`.
            Role::User | Role::Tool => {
                ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                    content: ChatCompletionRequestUserMessageContent::Text(content.clone()),
                    name: None,
                })
            }
        },
        ModelRequestItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => ChatCompletionRequestMessage::Assistant(ChatCompletionRequestAssistantMessage {
            tool_calls: Some(vec![ChatCompletionMessageToolCalls::Function(
                ChatCompletionMessageToolCall {
                    id: call_id.clone(),
                    function: FunctionCall {
                        name: name.clone(),
                        arguments: arguments.clone(),
                    },
                },
            )]),
            ..Default::default()
        }),
        ModelRequestItem::FunctionCallOutput { call_id, output } => {
            ChatCompletionRequestMessage::Tool(ChatCompletionRequestToolMessage {
                content: ChatCompletionRequestToolMessageContent::Text(output.clone()),
                tool_call_id: call_id.clone(),
            })
        }
    }
}

/// Converts a tool definition into the API's tool shape.
#[must_use]
pub fn to_tool(tool: &ToolDefinition) -> ChatCompletionTools {
    ChatCompletionTools::Function(ChatCompletionTool {
        function: FunctionObject {
            name: tool.name.clone(),
            description: Some(tool.description.clone()),
            parameters: Some(tool.parameters.clone()),
            strict: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a chunk the way the wire delivers it.
    fn chunk(value: serde_json::Value) -> StreamChunk {
        serde_json::from_value(serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "gpt-x",
            "choices": value,
        }))
        .expect("a chunk deserializes")
    }

    fn text(content: &str) -> serde_json::Value {
        serde_json::json!({
            "index": 0,
            "delta": { "content": content },
            "finish_reason": null,
        })
    }

    fn reasoning(content: &str) -> serde_json::Value {
        serde_json::json!({
            "index": 0,
            "delta": { "reasoning_content": content },
            "finish_reason": null,
        })
    }

    fn call(
        index: u32,
        id: Option<&str>,
        name: Option<&str>,
        arguments: &str,
    ) -> serde_json::Value {
        let function = match name {
            Some(name) => serde_json::json!({ "name": name, "arguments": arguments }),
            None => serde_json::json!({ "arguments": arguments }),
        };
        let mut entry = serde_json::json!({
            "index": index,
            "type": "function",
            "function": function,
        });
        if let Some(id) = id {
            entry["id"] = serde_json::Value::String(id.into());
        }
        serde_json::json!({
            "index": 0,
            "delta": { "tool_calls": [entry] },
            "finish_reason": null,
        })
    }

    #[test]
    fn text_chunks_become_text_events() {
        let mut calls = CallAssembly::default();
        assert_eq!(
            fold_chunk(chunk(serde_json::json!([text("po")])), &mut calls),
            vec![ModelEvent::TextDelta { delta: "po".into() }]
        );
    }

    #[test]
    fn empty_content_is_not_emitted() {
        let mut calls = CallAssembly::default();
        assert!(fold_chunk(chunk(serde_json::json!([text("")])), &mut calls).is_empty());
    }

    #[test]
    fn reasoning_chunks_become_thought_events() {
        let mut calls = CallAssembly::default();
        assert_eq!(
            fold_chunk(
                chunk(serde_json::json!([reasoning("let me think")])),
                &mut calls
            ),
            vec![ModelEvent::ThoughtDelta {
                delta: "let me think".into()
            }]
        );
    }

    #[test]
    fn empty_reasoning_is_not_emitted() {
        let mut calls = CallAssembly::default();
        assert!(fold_chunk(chunk(serde_json::json!([reasoning("")])), &mut calls).is_empty());
    }

    #[test]
    fn reasoning_precedes_text_in_a_chunk_carrying_both() {
        let mut calls = CallAssembly::default();
        let both = serde_json::json!({
            "index": 0,
            "delta": { "reasoning_content": "hmm", "content": "answer" },
            "finish_reason": null,
        });
        assert_eq!(
            fold_chunk(chunk(serde_json::json!([both])), &mut calls),
            vec![
                ModelEvent::ThoughtDelta {
                    delta: "hmm".into()
                },
                ModelEvent::TextDelta {
                    delta: "answer".into()
                },
            ]
        );
    }

    #[test]
    fn a_delta_with_an_empty_content_beside_reasoning_emits_only_the_thought() {
        let mut calls = CallAssembly::default();
        // The shape Qwen streams while it reasons: both keys are present and
        // the one not being written yet is empty.
        let both = serde_json::json!({
            "index": 0,
            "delta": { "content": "", "reasoning_content": "We" },
            "finish_reason": null,
        });
        assert_eq!(
            fold_chunk(chunk(serde_json::json!([both])), &mut calls),
            vec![ModelEvent::ThoughtDelta { delta: "We".into() }]
        );
    }

    #[test]
    fn a_chunk_whose_delta_carries_nothing_emits_nothing() {
        let mut calls = CallAssembly::default();
        let empty = serde_json::json!({ "index": 0, "delta": {}, "finish_reason": "stop" });
        assert!(fold_chunk(chunk(serde_json::json!([empty])), &mut calls).is_empty());
    }

    #[test]
    fn a_usage_only_chunk_carries_no_choices() {
        let mut calls = CallAssembly::default();
        let usage_only = serde_json::json!({
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "created": 0,
            "model": "gpt-x",
            "choices": [],
            "usage": { "total_tokens": 3 },
        });
        let parsed: StreamChunk = serde_json::from_value(usage_only).expect("a chunk deserializes");
        assert!(fold_chunk(parsed, &mut calls).is_empty());
    }

    #[test]
    fn a_choice_without_a_delta_is_tolerated() {
        let mut calls = CallAssembly::default();
        let no_delta = serde_json::json!({ "index": 0, "finish_reason": "stop" });
        assert!(fold_chunk(chunk(serde_json::json!([no_delta])), &mut calls).is_empty());
    }

    #[test]
    fn a_chunk_with_no_choices_carries_nothing() {
        let mut calls = CallAssembly::default();
        assert!(fold_chunk(chunk(serde_json::json!([])), &mut calls).is_empty());
    }

    #[test]
    fn a_tool_call_is_named_before_its_arguments_are_whole() {
        let mut calls = CallAssembly::default();
        // The id and name arrive first, the arguments in pieces afterwards. The
        // first fragment is therefore the moment the call can be shown, and the
        // rest of it is the moment it can be run.
        assert_eq!(
            fold_chunk(
                chunk(serde_json::json!([call(
                    0,
                    Some("call_1"),
                    Some("read_file"),
                    ""
                )])),
                &mut calls
            ),
            vec![ModelEvent::ToolCallNamed {
                call_id: "call_1".into(),
                name: "read_file".into(),
            }],
            "the name alone is forwarded, so a call is visible before it is runnable"
        );
        for arguments in [r#"{"pa"#, r#"th":"x"}"#] {
            assert!(
                fold_chunk(
                    chunk(serde_json::json!([call(0, None, None, arguments)])),
                    &mut calls
                )
                .is_empty(),
                "and nothing more: the call was already named"
            );
        }

        assert_eq!(
            calls.take(),
            vec![ModelEvent::ToolCall {
                call_id: "call_1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"x\"}".into(),
            }]
        );
    }

    #[test]
    fn a_call_is_named_once_however_many_chunks_repeat_its_name() {
        // DashScope repeats the id and name on every chunk of a call.
        let mut calls = CallAssembly::default();
        let mut named = 0;
        for chunk_index in 0..3 {
            named += fold_chunk(
                chunk(serde_json::json!([call(
                    0,
                    Some("call_1"),
                    Some("read_file"),
                    if chunk_index == 0 { "" } else { "x" }
                )])),
                &mut calls,
            )
            .len();
        }

        assert_eq!(named, 1, "one announcement, not one per chunk");
    }

    #[test]
    fn a_provider_that_repeats_an_empty_id_and_name_does_not_erase_them() {
        // DashScope's OpenAI-compatible endpoint sends the id and name on the
        // first chunk of a call and `""` on every chunk after it. Overwriting with
        // the later value left every call with no name, so the runtime dispatched
        // it and the registry answered that no tool had that name — which reads
        // like a wiring fault rather than as the protocol quirk it is, and sent the
        // model back to retry a call it had already made correctly.
        //
        // `Some("")`, not `None`: that is the shape the provider sends and `None`
        // is what the test above uses, which is exactly why that one passed here
        // for as long as it did.
        let mut calls = CallAssembly::default();
        fold_chunk(
            chunk(serde_json::json!([call(
                0,
                Some("call_3a2730d5"),
                Some("bash"),
                ""
            )])),
            &mut calls,
        );
        // What the provider actually sends afterwards.
        for piece in [r#"{"command":"#, r#""echo hi"}"#] {
            fold_chunk(
                chunk(serde_json::json!([call(0, Some(""), Some(""), piece)])),
                &mut calls,
            );
        }

        assert_eq!(
            calls.take(),
            vec![ModelEvent::ToolCall {
                call_id: "call_3a2730d5".into(),
                name: "bash".into(),
                arguments: r#"{"command":"echo hi"}"#.into(),
            }],
            "the first chunk's id and name survive the empty ones"
        );
    }

    #[test]
    fn a_call_that_never_got_a_name_is_dropped() {
        // The second line of defence, behind the empty-value guard: a call with no
        // name is forwarded to a registry that will answer "there is no tool by
        // that name", and the model then retries a call it had already made
        // correctly. Dropping it says nothing, which is better than saying the
        // wrong thing — and the turn still ends on whatever the model says next.
        let mut calls = CallAssembly::default();
        fold_chunk(
            chunk(serde_json::json!([call(
                0,
                Some(""),
                Some(""),
                r#"{"command":"#
            )])),
            &mut calls,
        );
        fold_chunk(
            chunk(serde_json::json!([call(0, Some(""), Some(""), r#""ls"}"#)])),
            &mut calls,
        );

        assert!(
            calls.take().is_empty(),
            "nothing the runtime could dispatch is forwarded"
        );
    }

    #[test]
    fn a_later_non_empty_name_still_replaces_an_earlier_one() {
        // The other half of the rule: a provider that corrects itself mid-stream
        // should be believed, so the guard is against emptiness and not against
        // repetition.
        let mut calls = CallAssembly::default();
        fold_chunk(
            chunk(serde_json::json!([call(
                0,
                Some("call_1"),
                Some("wrong_name"),
                ""
            )])),
            &mut calls,
        );
        fold_chunk(
            chunk(serde_json::json!([call(0, None, Some("right_name"), "")])),
            &mut calls,
        );

        match &calls.take()[0] {
            ModelEvent::ToolCall { name, .. } => assert_eq!(name, "right_name"),
            other => panic!("expected a tool call: {other:?}"),
        }
    }

    #[test]
    fn parallel_tool_calls_keep_their_own_arguments() {
        let mut calls = CallAssembly::default();
        fold_chunk(
            chunk(serde_json::json!([call(
                0,
                Some("call_1"),
                Some("read_file"),
                "{\"a\":1}"
            )])),
            &mut calls,
        );
        fold_chunk(
            chunk(serde_json::json!([call(
                1,
                Some("call_2"),
                Some("read_file"),
                "{\"b\":"
            )])),
            &mut calls,
        );
        fold_chunk(
            chunk(serde_json::json!([call(1, None, None, "2}")])),
            &mut calls,
        );

        assert_eq!(
            calls.take(),
            vec![
                ModelEvent::ToolCall {
                    call_id: "call_1".into(),
                    name: "read_file".into(),
                    arguments: "{\"a\":1}".into(),
                },
                ModelEvent::ToolCall {
                    call_id: "call_2".into(),
                    name: "read_file".into(),
                    arguments: "{\"b\":2}".into(),
                },
            ]
        );
    }

    #[test]
    fn a_drained_assembly_yields_nothing_further() {
        let mut calls = CallAssembly::default();
        fold_chunk(
            chunk(serde_json::json!([call(
                0,
                Some("call_1"),
                Some("read_file"),
                "{}"
            )])),
            &mut calls,
        );
        assert_eq!(calls.take().len(), 1);
        assert!(calls.take().is_empty());
    }

    #[test]
    fn request_messages_map_onto_the_api_shape() {
        let request = ModelRequest {
            items: vec![
                ModelRequestItem::Message {
                    role: Role::User,
                    content: "hi".into(),
                },
                ModelRequestItem::FunctionCall {
                    call_id: "c1".into(),
                    name: "read_file".into(),
                    arguments: "{}".into(),
                },
                ModelRequestItem::FunctionCallOutput {
                    call_id: "c1".into(),
                    output: "contents".into(),
                },
            ],
            tools: Vec::new(),
            instructions: Some("be brief".into()),
        };

        let sdk_request = to_request("gpt-x", &request);
        assert_eq!(sdk_request.model, "gpt-x");
        // The system instruction is a message here, not a separate field.
        assert_eq!(sdk_request.messages.len(), 4);

        let messages = serde_json::to_value(&sdk_request.messages).expect("messages serialize");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be brief");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["tool_calls"][0]["id"], "c1");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "c1");
    }

    #[test]
    fn a_request_without_instructions_has_no_system_message() {
        let request = ModelRequest {
            items: vec![ModelRequestItem::Message {
                role: Role::User,
                content: "hi".into(),
            }],
            tools: Vec::new(),
            instructions: None,
        };

        let sdk_request = to_request("gpt-x", &request);
        let messages = serde_json::to_value(&sdk_request.messages).expect("messages serialize");
        assert_eq!(messages[0]["role"], "user");
    }

    #[test]
    fn the_request_asks_for_a_stream() {
        let request = ModelRequest {
            items: Vec::new(),
            tools: Vec::new(),
            instructions: None,
        };

        assert_eq!(to_request("gpt-x", &request).stream, Some(true));
    }

    #[test]
    fn a_tool_definition_maps_onto_the_api_shape() {
        let definition = ToolDefinition {
            name: "read_file".into(),
            description: "Reads a file.".into(),
            parameters: serde_json::json!({ "type": "object" }),
        };
        let json = serde_json::to_value(to_tool(&definition)).expect("tool serializes");
        assert_eq!(json["type"], "function");
        assert_eq!(json["function"]["name"], "read_file");
        assert!(json["function"]["parameters"].is_object());
    }
}
