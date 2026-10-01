//! OpenAI Responses API adapter.
//!
//! Translates a Responses API stream into the core's [`ModelEvent`]
//! vocabulary. The mapping is pure so it can be tested without a network.

use async_openai::types::responses::{
    CreateResponse, CreateResponseArgs, EasyInputContent, EasyInputMessage, FunctionCallOutput,
    FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, InputItem, InputParam, Item,
    MessageType, OutputItem, ResponseStreamEvent, ResponseTextDeltaEvent, Role as ApiRole, Tool,
};
use async_openai::{config::OpenAIConfig, Client};
use futures::StreamExt;
use srud_core::client::{
    ModelClient, ModelError, ModelEvent, ModelRequest, ModelRequestItem, ModelStream,
};
use srud_core::tools::ToolDefinition;
use srud_core::types::Role;
use tokio_util::sync::CancellationToken;

use super::{connect, ClientConfig};

/// A client for the Responses API.
///
/// Holds the provider SDK's client and the model id, and implements the core's
/// [`ModelClient`] by streaming a response and translating each event.
pub struct ResponsesClient {
    client: Client<OpenAIConfig>,
    model: String,
}

impl ResponsesClient {
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
impl ModelClient for ResponsesClient {
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        let sdk_request = to_request(&self.model, &request);

        let mut stream = self
            .client
            .responses()
            .create_stream(sdk_request)
            .await
            .map_err(|error| ModelError::Rejected(error.to_string()))?;

        // The translation is a pure step applied to each provider event, so the
        // stream shape the core sees does not depend on the provider's.
        let translated = async_stream::stream! {
            let mut pending = None;
            loop {
                if cancel.is_cancelled() {
                    return;
                }
                let Some(next) = stream.next().await else {
                    return;
                };
                let event = match next {
                    Ok(event) => event,
                    Err(error) => {
                        yield Err(ModelError::Transport(error.to_string()));
                        return;
                    }
                };
                if let Some(item) = into_stream_item(translate(event, &mut pending)) {
                    yield item;
                }
            }
        };

        Ok(Box::pin(translated))
    }
}

/// A function call being assembled from a stream.
///
/// The call's identity arrives on the item-added event and its arguments arrive
/// as a delta stream, so neither alone is enough to run it.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PendingCall {
    call_id: String,
    name: String,
    arguments: String,
}

/// What one stream event means for the caller.
#[derive(Debug, Clone, PartialEq)]
enum Translated {
    /// Nothing observable happened.
    Ignore,
    /// Forward this to the core.
    Emit(ModelEvent),
    /// A function call is complete and ready to run.
    CallReady(ModelEvent),
    /// The provider reported a failure.
    Fail(String),
}

/// Builds the SDK request from a core request.
#[must_use]
pub fn to_request(model: &str, request: &ModelRequest) -> CreateResponse {
    let items = request.items.iter().map(to_input_item).collect();

    CreateResponseArgs::default()
        .model(model)
        .input(InputParam::Items(items))
        .instructions(request.instructions.clone().unwrap_or_default())
        .tools(request.tools.iter().map(to_tool).collect::<Vec<_>>())
        // The `byot` feature this workspace enables removes the SDK's own
        // stream flag handling, so the request has to ask for a stream itself.
        .stream(true)
        .build()
        .unwrap_or_default()
}

/// Maps one recorded request item onto an API input item.
fn to_input_item(item: &ModelRequestItem) -> InputItem {
    match item {
        ModelRequestItem::Message { role, content } => InputItem::EasyMessage(EasyInputMessage {
            r#type: MessageType::Message,
            role: to_api_role(*role),
            content: EasyInputContent::Text(content.clone()),
            phase: None,
        }),
        ModelRequestItem::FunctionCall {
            call_id,
            name,
            arguments,
        } => InputItem::Item(Item::FunctionCall(FunctionToolCall {
            arguments: arguments.clone(),
            call_id: call_id.clone(),
            namespace: None,
            name: name.clone(),
            id: None,
            status: None,
            caller: None,
            r#async: None,
        })),
        ModelRequestItem::FunctionCallOutput { call_id, output } => {
            InputItem::Item(Item::FunctionCallOutput(FunctionCallOutputItemParam {
                call_id: Some(call_id.clone()),
                output: FunctionCallOutput::Text(output.clone()),
                id: None,
                status: None,
                name: None,
                namespace: None,
                caller: None,
            }))
        }
    }
}

/// Maps a core role onto an API role.
///
/// Tool output is carried by [`ModelRequestItem::FunctionCallOutput`], so a
/// `Tool`-role message is folded into the user role.
fn to_api_role(role: Role) -> ApiRole {
    match role {
        Role::Assistant => ApiRole::Assistant,
        Role::User | Role::Tool => ApiRole::User,
    }
}

/// Converts a tool definition into the API's tool shape.
#[must_use]
pub fn to_tool(tool: &ToolDefinition) -> Tool {
    Tool::Function(FunctionTool {
        name: tool.name.clone(),
        description: Some(tool.description.clone()),
        parameters: Some(tool.parameters.clone()),
        strict: None,
        defer_loading: None,
        r#async: None,
        output_schema: None,
        allowed_callers: None,
    })
}

/// Translates one stream event.
///
/// `pending` carries call state between events, since arguments arrive across
/// several of them.
fn translate(event: ResponseStreamEvent, pending: &mut Option<PendingCall>) -> Translated {
    match event {
        ResponseStreamEvent::ResponseOutputTextDelta(ResponseTextDeltaEvent { delta, .. }) => {
            Translated::Emit(ModelEvent::TextDelta { delta })
        }
        ResponseStreamEvent::ResponseReasoningSummaryTextDelta(delta) => {
            Translated::Emit(ModelEvent::ThoughtDelta { delta: delta.delta })
        }
        ResponseStreamEvent::ResponseReasoningTextDelta(delta) => {
            Translated::Emit(ModelEvent::ThoughtDelta { delta: delta.delta })
        }
        ResponseStreamEvent::ResponseOutputItemAdded(added) => {
            if let OutputItem::FunctionCall(call) = added.item {
                *pending = Some(PendingCall {
                    call_id: call.call_id,
                    name: call.name,
                    arguments: String::new(),
                });
            }
            Translated::Ignore
        }
        ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(delta) => {
            if let Some(call) = pending.as_mut() {
                call.arguments.push_str(&delta.delta);
            }
            Translated::Ignore
        }
        ResponseStreamEvent::ResponseFunctionCallArgumentsDone(done) => {
            let mut call = pending.take().unwrap_or_default();
            if let Some(name) = done.name {
                call.name = name;
            }
            if call.arguments.is_empty() {
                call.arguments = done.arguments;
            }
            Translated::CallReady(ModelEvent::ToolCall {
                call_id: call.call_id,
                name: call.name,
                arguments: call.arguments,
            })
        }
        ResponseStreamEvent::ResponseCompleted(_) => Translated::Emit(ModelEvent::Done),
        ResponseStreamEvent::ResponseFailed(failed) => Translated::Fail(
            failed
                .response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "response failed".to_owned()),
        ),
        ResponseStreamEvent::ResponseIncomplete(_) => {
            Translated::Fail("response ended incomplete".to_owned())
        }
        _ => Translated::Ignore,
    }
}

/// Adapts a translated event into a stream item.
fn into_stream_item(translated: Translated) -> Option<Result<ModelEvent, ModelError>> {
    match translated {
        Translated::Ignore => None,
        Translated::Emit(event) | Translated::CallReady(event) => Some(Ok(event)),
        Translated::Fail(message) => Some(Err(ModelError::Transport(message))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_openai::types::responses::{
        ResponseFunctionCallArgumentsDeltaEvent, ResponseFunctionCallArgumentsDoneEvent,
        ResponseOutputItemAddedEvent, ResponseReasoningSummaryTextDeltaEvent,
    };

    fn text_delta(delta: &str) -> ResponseStreamEvent {
        ResponseStreamEvent::ResponseOutputTextDelta(ResponseTextDeltaEvent {
            sequence_number: 1,
            item_id: "item_1".into(),
            output_index: 0,
            content_index: 0,
            delta: delta.into(),
            logprobs: None,
        })
    }

    fn function_call_item(call_id: &str, name: &str) -> ResponseStreamEvent {
        ResponseStreamEvent::ResponseOutputItemAdded(ResponseOutputItemAddedEvent {
            sequence_number: 1,
            output_index: 0,
            item: OutputItem::FunctionCall(FunctionToolCall {
                arguments: String::new(),
                call_id: call_id.into(),
                namespace: None,
                name: name.into(),
                id: None,
                status: None,
                caller: None,
                r#async: None,
            }),
        })
    }

    fn args_delta(delta: &str) -> ResponseStreamEvent {
        ResponseStreamEvent::ResponseFunctionCallArgumentsDelta(
            ResponseFunctionCallArgumentsDeltaEvent {
                sequence_number: 2,
                item_id: "item_1".into(),
                output_index: 0,
                delta: delta.into(),
            },
        )
    }

    fn args_done(arguments: &str) -> ResponseStreamEvent {
        ResponseStreamEvent::ResponseFunctionCallArgumentsDone(
            ResponseFunctionCallArgumentsDoneEvent {
                name: Some("read_file".into()),
                sequence_number: 3,
                item_id: "item_1".into(),
                output_index: 0,
                arguments: arguments.into(),
            },
        )
    }

    #[test]
    fn text_deltas_become_text_events() {
        let mut pending = None;
        let translated = translate(text_delta("hi"), &mut pending);
        assert_eq!(
            translated,
            Translated::Emit(ModelEvent::TextDelta { delta: "hi".into() })
        );
    }

    #[test]
    fn reasoning_deltas_become_thought_events() {
        let mut pending = None;
        let event = ResponseStreamEvent::ResponseReasoningSummaryTextDelta(
            ResponseReasoningSummaryTextDeltaEvent {
                sequence_number: 1,
                item_id: "item_1".into(),
                output_index: 0,
                summary_index: 0,
                delta: "thinking".into(),
            },
        );
        assert_eq!(
            translate(event, &mut pending),
            Translated::Emit(ModelEvent::ThoughtDelta {
                delta: "thinking".into()
            })
        );
    }

    #[test]
    fn a_function_call_accumulates_arguments_across_events() {
        let mut pending = None;
        assert_eq!(
            translate(function_call_item("call_1", "read_file"), &mut pending),
            Translated::Ignore
        );
        assert_eq!(
            translate(args_delta("{\"pa"), &mut pending),
            Translated::Ignore
        );
        assert_eq!(
            translate(args_delta("th\":\"x\"}"), &mut pending),
            Translated::Ignore
        );

        assert_eq!(
            translate(args_done(""), &mut pending),
            Translated::CallReady(ModelEvent::ToolCall {
                call_id: "call_1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"x\"}".into(),
            })
        );
        assert!(pending.is_none(), "the call is consumed once it is ready");
    }

    #[test]
    fn a_function_call_falls_back_to_the_done_arguments() {
        let mut pending = None;
        translate(function_call_item("call_1", "read_file"), &mut pending);
        // No delta events arrived; the arguments come in on the done event.
        assert_eq!(
            translate(args_done("{\"path\":\"y\"}"), &mut pending),
            Translated::CallReady(ModelEvent::ToolCall {
                call_id: "call_1".into(),
                name: "read_file".into(),
                arguments: "{\"path\":\"y\"}".into(),
            })
        );
    }

    #[test]
    fn completion_becomes_done() {
        let mut pending = None;
        let event = ResponseStreamEvent::ResponseCompleted(
            async_openai::types::responses::ResponseCompletedEvent {
                sequence_number: 9,
                response: sample_response(),
            },
        );
        assert_eq!(
            translate(event, &mut pending),
            Translated::Emit(ModelEvent::Done)
        );
    }

    #[test]
    fn a_failure_becomes_a_stream_error() {
        let mut pending = None;
        let event = ResponseStreamEvent::ResponseIncomplete(
            async_openai::types::responses::ResponseIncompleteEvent {
                sequence_number: 9,
                response: sample_response(),
            },
        );
        let item = into_stream_item(translate(event, &mut pending)).expect("an item");
        assert!(matches!(item, Err(ModelError::Transport(_))));
    }

    #[test]
    fn unmodelled_events_are_ignored() {
        let mut pending = None;
        let event = ResponseStreamEvent::ResponseInProgress(
            async_openai::types::responses::ResponseInProgressEvent {
                sequence_number: 1,
                response: sample_response(),
            },
        );
        assert_eq!(translate(event, &mut pending), Translated::Ignore);
        assert!(into_stream_item(Translated::Ignore).is_none());
    }

    #[test]
    fn a_tool_definition_maps_onto_the_api_shape() {
        let definition = srud_core::tools::ToolDefinition {
            name: "read_file".into(),
            description: "Reads a file.".into(),
            parameters: serde_json::json!({ "type": "object" }),
        };
        match to_tool(&definition) {
            Tool::Function(tool) => {
                assert_eq!(tool.name, "read_file");
                assert_eq!(tool.description.as_deref(), Some("Reads a file."));
                assert!(tool.parameters.is_some());
            }
            other => panic!("expected a function tool, got {other:?}"),
        }
    }

    #[test]
    fn request_items_map_onto_input_items() {
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
        assert_eq!(sdk_request.model.as_deref(), Some("gpt-x"));
        assert_eq!(sdk_request.instructions.as_deref(), Some("be brief"));
        match sdk_request.input {
            InputParam::Items(items) => assert_eq!(items.len(), 3),
            InputParam::Text(_) => panic!("expected items"),
        }
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

    fn sample_response() -> async_openai::types::responses::Response {
        serde_json::from_value(serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 0,
            "model": "gpt-x",
            "status": "completed",
            "output": []
        }))
        .expect("a minimal response deserializes")
    }
}
