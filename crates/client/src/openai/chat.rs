//! OpenAI Chat Completions adapter.
//!
//! Translates a `/chat/completions` stream into the core's [`ModelEvent`]
//! vocabulary. The stream is flatter than the Responses API's: every chunk
//! carries a list of choices whose deltas hold whatever arrived next.

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
            calls.absorb(fragment);
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
/// A call arrives across several chunks keyed by index. The id and name may
/// arrive in one chunk and the arguments spread over the rest, so no single
/// chunk is the call.
#[derive(Debug, Default)]
struct CallAssembly {
    calls: BTreeMap<u32, PendingCall>,
}

impl CallAssembly {
    fn absorb(&mut self, fragment: ChatCompletionMessageToolCallChunk) {
        let call = self.calls.entry(fragment.index).or_default();
        if let Some(id) = fragment.id {
            call.call_id = id;
        }
        if let Some(function) = fragment.function {
            if let Some(name) = function.name {
                call.name = name;
            }
            if let Some(arguments) = function.arguments {
                call.arguments.push_str(&arguments);
            }
        }
    }

    /// Drains the assembled calls in index order.
    fn take(&mut self) -> Vec<ModelEvent> {
        std::mem::take(&mut self.calls)
            .into_values()
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
    fn a_tool_call_accumulates_arguments_across_chunks() {
        let mut calls = CallAssembly::default();
        // The id and name arrive first, the arguments in pieces afterwards.
        assert!(fold_chunk(
            chunk(serde_json::json!([call(
                0,
                Some("call_1"),
                Some("read_file"),
                ""
            )])),
            &mut calls
        )
        .is_empty());
        assert!(fold_chunk(
            chunk(serde_json::json!([call(0, None, None, "{\"pa")])),
            &mut calls
        )
        .is_empty());
        assert!(fold_chunk(
            chunk(serde_json::json!([call(0, None, None, "th\":\"x\"}")])),
            &mut calls
        )
        .is_empty());

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
