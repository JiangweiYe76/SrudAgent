//! OpenAI-compatible endpoints.
//!
//! Two endpoints are supported. They share the OpenAI wire format but not their
//! request shape or stream events, so each gets its own client:
//!
//! - [`ResponsesClient`] — `POST /responses`
//! - [`ChatClient`] — `POST /chat/completions`

mod chat;
mod responses;

pub use chat::ChatClient;
pub use responses::ResponsesClient;

use async_openai::config::OpenAIConfig;
use async_openai::Client;
use srud_core::client::ModelError;

/// The default model, used when the caller does not name one.
pub const DEFAULT_MODEL: &str = "gpt-5";

/// Connection settings for an OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Bearer token sent with every request.
    pub api_key: String,
    /// Endpoint root, with no trailing slash. Include whatever version segment
    /// the provider expects — for OpenAI itself that is
    /// `https://api.openai.com/v1`. `None` falls back to the SDK default.
    pub base_url: Option<String>,
    /// Model id sent with every request.
    pub model: String,
}

impl ClientConfig {
    /// Environment variable holding the API key.
    pub const API_KEY_VAR: &'static str = "SRUD_API_KEY";
    /// Environment variable holding the endpoint root.
    pub const BASE_URL_VAR: &'static str = "SRUD_BASE_URL";
    /// Environment variable holding the model id.
    pub const MODEL_VAR: &'static str = "SRUD_MODEL";

    /// Creates settings with an explicit key and model.
    #[must_use]
    pub fn new(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            base_url: None,
            model: model.into(),
        }
    }

    /// Points the client at a specific endpoint.
    #[must_use]
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    /// Reads settings from the environment.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::Config`] if the API key is unset or blank. The
    /// base URL is optional, and the model falls back to [`DEFAULT_MODEL`].
    pub fn from_env() -> Result<Self, ModelError> {
        let api_key = env_var(Self::API_KEY_VAR)
            .ok_or_else(|| ModelError::Config(format!("{} is not set", Self::API_KEY_VAR)))?;

        Ok(Self {
            api_key,
            base_url: env_var(Self::BASE_URL_VAR),
            model: env_var(Self::MODEL_VAR).unwrap_or_else(|| DEFAULT_MODEL.to_owned()),
        })
    }
}

/// Reads a variable, treating blank as unset so an empty line in a dotenv file
/// behaves the same as omitting it.
fn env_var(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// Builds the SDK client and the model id from settings.
fn connect(settings: ClientConfig) -> (Client<OpenAIConfig>, String) {
    let mut config = OpenAIConfig::new().with_api_key(settings.api_key);
    if let Some(base_url) = settings.base_url {
        config = config.with_api_base(base_url);
    }
    (Client::with_config(config), settings.model)
}
