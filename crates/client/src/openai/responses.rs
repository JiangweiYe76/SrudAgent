//! OpenAI Responses API adapter.
//!
//! Translates a Responses API stream into the core's [`ModelEvent`]
//! vocabulary. The mapping is pure so it can be tested without a network.

use async_openai::types::responses::{
    CreateResponse, CreateResponseArgs, EasyInputContent, EasyInputMessage, FunctionCallOutput,
    FunctionCallOutputItemParam, FunctionTool, FunctionToolCall, ImageDetail, InputContent,
    InputImageContent, InputItem, InputParam, InputTextContent, Item, MessageType, OutputItem,
    ResponseStreamEvent, ResponseTextDeltaEvent, ResponseUsage, Role as ApiRole, Tool,
};
use async_openai::{config::OpenAIConfig, Client};
use futures::StreamExt;
use srud_core::client::{
    ModelClient, ModelError, ModelEvent, ModelRequest, ModelRequestItem, ModelStream, TokenUsage,
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
                for item in into_stream_items(translate(event, &mut pending)) {
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
    /// The provider reported the request's cost, then completed.
    UsageDone(TokenUsage),
    /// The provider reported the cost of a request that ended badly.
    ///
    /// A failed or incomplete response still charged for what it produced;
    /// dropping the report here would make the cost invisible.
    UsageFail(TokenUsage, String),
}

/// Builds the SDK request from a core request.
#[must_use]
pub fn to_request(model: &str, request: &ModelRequest) -> CreateResponse {
    let items = to_input_items(&request.items);

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

/// Maps recorded request items onto API input items.
///
/// A user's text and the images sent with it are recorded as separate items,
/// but the API reads them as one message, so consecutive user items are folded
/// into a single message whose content is a part list.
fn to_input_items(items: &[ModelRequestItem]) -> Vec<InputItem> {
    let mut input = Vec::with_capacity(items.len());
    for item in items {
        match item {
            ModelRequestItem::Message {
                role: Role::User | Role::Tool,
                content,
            } => {
                push_user_content(&mut input, text_content(content));
            }
            ModelRequestItem::Image { mime, data } => {
                push_user_content(&mut input, image_content(mime, data));
            }
            item => input.push(to_input_item(item)),
        }
    }
    input
}

/// Appends a part to the trailing user message, starting one if the last
/// message belongs to another role.
fn push_user_content(input: &mut Vec<InputItem>, content: InputContent) {
    if let Some(InputItem::EasyMessage(message)) = input.last_mut() {
        if message.role == ApiRole::User {
            let mut parts = match std::mem::replace(
                &mut message.content,
                EasyInputContent::Text(String::new()),
            ) {
                EasyInputContent::Text(text) if text.is_empty() => Vec::new(),
                EasyInputContent::Text(text) => vec![text_content(&text)],
                EasyInputContent::ContentList(parts) => parts,
            };
            parts.push(content);
            message.content = EasyInputContent::ContentList(parts);
            return;
        }
    }
    input.push(InputItem::EasyMessage(EasyInputMessage {
        r#type: MessageType::Message,
        role: ApiRole::User,
        content: EasyInputContent::ContentList(vec![content]),
        phase: None,
    }));
}

fn text_content(text: &str) -> InputContent {
    InputContent::InputText(InputTextContent {
        text: text.to_owned(),
        prompt_cache_breakpoint: None,
    })
}

fn image_content(mime: &str, data: &str) -> InputContent {
    InputContent::InputImage(InputImageContent {
        detail: ImageDetail::default(),
        file_id: None,
        image_url: Some(format!("data:{mime};base64,{data}")),
        prompt_cache_breakpoint: None,
    })
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
        ModelRequestItem::Image { mime, data } => InputItem::EasyMessage(EasyInputMessage {
            r#type: MessageType::Message,
            role: ApiRole::User,
            content: EasyInputContent::ContentList(vec![image_content(mime, data)]),
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
                let call_id = call.call_id;
                let name = call.name;
                *pending = Some(PendingCall {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: String::new(),
                });
                // This event is where the name arrives and the arguments have not,
                // which is the only moment a call can be shown but not yet run.
                if !name.is_empty() {
                    return Translated::Emit(ModelEvent::ToolCallNamed { call_id, name });
                }
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
        ResponseStreamEvent::ResponseCompleted(completed) => {
            match completed.response.usage.as_ref().map(to_token_usage) {
                Some(usage) => Translated::UsageDone(usage),
                None => Translated::Emit(ModelEvent::Done),
            }
        }
        ResponseStreamEvent::ResponseFailed(failed) => fail_translated(
            failed.response.usage.as_ref(),
            failed
                .response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "response failed".to_owned()),
        ),
        ResponseStreamEvent::ResponseIncomplete(incomplete) => fail_translated(
            incomplete.response.usage.as_ref(),
            "response ended incomplete".to_owned(),
        ),
        _ => Translated::Ignore,
    }
}

/// A failure, carrying the reported cost of the request first if there was one.
fn fail_translated(usage: Option<&ResponseUsage>, message: String) -> Translated {
    match usage {
        Some(usage) => Translated::UsageFail(to_token_usage(usage), message),
        None => Translated::Fail(message),
    }
}

/// Maps the endpoint's usage onto the core's counts.
///
/// A negative cache-write count — which the endpoint's signed field allows but
/// no provider is known to send — reads as zero, same as a missing one.
fn to_token_usage(usage: &ResponseUsage) -> TokenUsage {
    TokenUsage {
        input_tokens: u64::from(usage.input_tokens),
        output_tokens: u64::from(usage.output_tokens),
        cached_input_tokens: u64::from(usage.input_tokens_details.cached_tokens),
        cache_write_input_tokens: usage
            .input_tokens_details
            .cache_write_tokens
            .and_then(|count| u64::try_from(count).ok())
            .unwrap_or(0),
        reasoning_output_tokens: u64::from(usage.output_tokens_details.reasoning_tokens),
    }
}

/// Adapts a translated event into the stream items it produces.
///
/// Most events produce one; a completion or failure that carries a usage report
/// produces the report and then the terminator, so the cost is seen before the
/// step ends.
fn into_stream_items(translated: Translated) -> Vec<Result<ModelEvent, ModelError>> {
    match translated {
        Translated::Ignore => Vec::new(),
        Translated::Emit(event) | Translated::CallReady(event) => vec![Ok(event)],
        Translated::Fail(message) => vec![Err(ModelError::Transport(message))],
        Translated::UsageDone(usage) => vec![Ok(ModelEvent::Usage(usage)), Ok(ModelEvent::Done)],
        Translated::UsageFail(usage, message) => {
            vec![
                Ok(ModelEvent::Usage(usage)),
                Err(ModelError::Transport(message)),
            ]
        }
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
    fn user_text_and_images_fold_into_one_input_message() {
        let items = vec![
            ModelRequestItem::Message {
                role: Role::User,
                content: "look at this".into(),
            },
            ModelRequestItem::Image {
                mime: "image/png".into(),
                data: "AAAA".into(),
            },
            ModelRequestItem::Message {
                role: Role::Assistant,
                content: "I see it".into(),
            },
            ModelRequestItem::Message {
                role: Role::User,
                content: "thanks".into(),
            },
        ];

        let input = to_input_items(&items);
        assert_eq!(input.len(), 3);
        let value = serde_json::to_value(&input).expect("input serializes");
        assert_eq!(value[0]["role"], "user");
        assert_eq!(value[0]["content"][0]["type"], "input_text");
        assert_eq!(value[0]["content"][0]["text"], "look at this");
        assert_eq!(value[0]["content"][1]["type"], "input_image");
        assert_eq!(
            value[0]["content"][1]["image_url"],
            "data:image/png;base64,AAAA"
        );
        assert_eq!(value[1]["role"], "assistant");
        assert_eq!(value[2]["content"][0]["text"], "thanks");
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
    fn a_function_call_is_named_before_its_arguments_are_whole() {
        let mut pending = None;
        // The item-added event is where the name arrives and the arguments have
        // not, which is the only moment the call can be shown but not yet run.
        assert_eq!(
            translate(function_call_item("call_1", "read_file"), &mut pending),
            Translated::Emit(ModelEvent::ToolCallNamed {
                call_id: "call_1".into(),
                name: "read_file".into(),
            })
        );
        for arguments in [r#"{"pa"#, r#"th":"x"}"#] {
            assert_eq!(
                translate(args_delta(arguments), &mut pending),
                Translated::Ignore,
                "and nothing more: the call was already named"
            );
        }

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
        let items = into_stream_items(translate(event, &mut pending));
        assert!(matches!(items.last(), Some(Err(ModelError::Transport(_)))));
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
        assert!(into_stream_items(Translated::Ignore).is_empty());
    }

    /// The usage a completed, failed, or incomplete response can carry.
    fn reported_usage() -> serde_json::Value {
        serde_json::json!({
            "input_tokens": 120,
            "input_tokens_details": { "cached_tokens": 100, "cache_write_tokens": 20 },
            "output_tokens": 30,
            "output_tokens_details": { "reasoning_tokens": 8 },
            "total_tokens": 150,
        })
    }

    fn response_with_usage() -> async_openai::types::responses::Response {
        serde_json::from_value(serde_json::json!({
            "id": "resp_1",
            "object": "response",
            "created_at": 0,
            "model": "gpt-x",
            "status": "completed",
            "output": [],
            "usage": reported_usage(),
        }))
        .expect("a response with usage deserializes")
    }

    const REPORTED: TokenUsage = TokenUsage {
        input_tokens: 120,
        output_tokens: 30,
        cached_input_tokens: 100,
        cache_write_input_tokens: 20,
        reasoning_output_tokens: 8,
    };

    #[test]
    fn a_completed_response_with_usage_reports_cost_before_done() {
        let mut pending = None;
        let event = ResponseStreamEvent::ResponseCompleted(
            async_openai::types::responses::ResponseCompletedEvent {
                sequence_number: 9,
                response: response_with_usage(),
            },
        );
        assert_eq!(
            into_stream_items(translate(event, &mut pending)),
            vec![Ok(ModelEvent::Usage(REPORTED)), Ok(ModelEvent::Done)],
            "the cost is seen before the step ends"
        );
    }

    #[test]
    fn a_failed_response_still_reports_its_cost() {
        let mut pending = None;
        let event = ResponseStreamEvent::ResponseIncomplete(
            async_openai::types::responses::ResponseIncompleteEvent {
                sequence_number: 9,
                response: response_with_usage(),
            },
        );
        let items = into_stream_items(translate(event, &mut pending));
        assert_eq!(items.len(), 2, "usage, then the failure: {items:?}");
        assert_eq!(items[0], Ok(ModelEvent::Usage(REPORTED)));
        assert!(matches!(items[1], Err(ModelError::Transport(_))));
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
