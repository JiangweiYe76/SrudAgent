//! The model seam.
//!
//! [`ModelClient`] is the only thing the turn loop knows about the model.
//!
//! [`ModelEvent`] is deliberately *not* [`Event`](crate::types::Event): a
//! `ModelEvent` mirrors one provider stream message, while an `Event` is a
//! semantic statement about the turn. Provider changes are absorbed here.

use std::pin::Pin;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use crate::tools::ToolDefinition;

/// A request to the model.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// The conversation so far, oldest first.
    pub items: Vec<ModelRequestItem>,
    /// Tools the model may call.
    pub tools: Vec<ToolDefinition>,
    /// Optional system-level instruction.
    pub instructions: Option<String>,
}

/// One item in a model request.
///
/// This mirrors [`ResponseItem`](crate::types::ResponseItem) but is separate:
/// the request shape is what a provider adapter needs.
#[derive(Debug, Clone)]
pub enum ModelRequestItem {
    /// A user, assistant, or tool message.
    Message {
        role: crate::types::Role,
        content: String,
    },
    /// A tool call the model previously made.
    FunctionCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// Output of a previously made tool call.
    FunctionCallOutput { call_id: String, output: String },
}

/// One message from the model's stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelEvent {
    /// A chunk of assistant-visible text.
    TextDelta { delta: String },
    /// A chunk of reasoning text.
    ThoughtDelta { delta: String },
    /// A complete tool call.
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    /// The model finished this response.
    Done,
}

/// Boxed stream of model events, so the trait stays object-safe.
pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ModelError>> + Send>>;

/// A model provider.
///
/// Implementations are shared across turns, so they must be `Send + Sync`.
#[async_trait::async_trait]
pub trait ModelClient: Send + Sync {
    /// Starts a streaming request.
    ///
    /// The returned stream yields events in provider order. Cancellation is
    /// cooperative through `cancel`.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError`] if the request could not be started. Failures
    /// after the stream is running arrive as `Err` items.
    async fn stream(
        &self,
        request: ModelRequest,
        cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError>;
}

/// A model call failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModelError {
    /// The provider rejected the request.
    #[error("model request rejected: {0}")]
    Rejected(String),

    /// The transport failed.
    #[error("model transport failed: {0}")]
    Transport(String),

    /// The stream ended before the model signalled completion.
    #[error("model stream ended unexpectedly")]
    Truncated,

    /// The client is missing settings it needs to make a request.
    #[error("model client is not configured: {0}")]
    Config(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;
    use futures::stream;

    /// A scripted client: hands back a fixed list of events.
    struct Scripted {
        events: Vec<Result<ModelEvent, ModelError>>,
    }

    #[async_trait::async_trait]
    impl ModelClient for Scripted {
        async fn stream(
            &self,
            _request: ModelRequest,
            _cancel: CancellationToken,
        ) -> Result<ModelStream, ModelError> {
            let events = self.events.clone();
            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn a_scripted_client_yields_its_events_in_order() {
        let client = Scripted {
            events: vec![
                Ok(ModelEvent::TextDelta { delta: "he".into() }),
                Ok(ModelEvent::TextDelta {
                    delta: "llo".into(),
                }),
                Ok(ModelEvent::Done),
            ],
        };

        let request = ModelRequest {
            items: vec![ModelRequestItem::Message {
                role: Role::User,
                content: "hi".into(),
            }],
            tools: Vec::new(),
            instructions: None,
        };

        let mut stream = client
            .stream(request, CancellationToken::new())
            .await
            .expect("stream starts");

        let mut text = String::new();
        while let Some(event) = futures::StreamExt::next(&mut stream).await {
            match event.expect("no error") {
                ModelEvent::TextDelta { delta } => text.push_str(&delta),
                ModelEvent::Done => break,
                ModelEvent::ThoughtDelta { .. } | ModelEvent::ToolCall { .. } => {}
            }
        }
        assert_eq!(text, "hello");
    }

    #[tokio::test]
    async fn errors_arrive_as_stream_items() {
        let client = Scripted {
            events: vec![Err(ModelError::Truncated)],
        };
        let request = ModelRequest {
            items: Vec::new(),
            tools: Vec::new(),
            instructions: None,
        };
        let mut stream = client
            .stream(request, CancellationToken::new())
            .await
            .expect("stream starts");
        let first = futures::StreamExt::next(&mut stream)
            .await
            .expect("one item");
        assert!(matches!(first, Err(ModelError::Truncated)));
    }
}
