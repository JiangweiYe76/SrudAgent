//! Provider administration over `_srud/unstable/provider/*`.
//!
//! Keys are masked on the way out: a client is shown enough to tell two keys
//! apart, and is told whether one exists at all.

use super::{parse_params, serialize, Agent};
use crate::providers::{self, ProviderError, ProviderStore};
use serde_json::Value;
use srud_protocol::acp::AcpError;
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};
use srud_protocol::srud::methods::{
    DeleteProviderRequest, DeleteProviderResponse, ListProvidersRequest, ListProvidersResponse,
    ProviderCatalog, SetDefaultProviderRequest, SetDefaultProviderResponse, SetProviderRequest,
    SetProviderResponse,
};
use std::sync::Arc;

impl Agent {
    /// The provider configuration, when the host supplied one.
    pub(super) fn provider_store(&self) -> Result<&Arc<ProviderStore>, AcpError> {
        self.providers
            .as_ref()
            .map(|providers| &providers.store)
            .ok_or_else(|| {
                AcpError::new(
                    METHOD_NOT_FOUND,
                    "this agent was not given a provider configuration",
                )
            })
    }

    pub(super) fn handle_provider_list(&self, params: Value) -> Result<Value, AcpError> {
        let _: ListProvidersRequest = parse_params(params)?;
        let store = self.provider_store()?;
        Ok(serialize(ListProvidersResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    pub(super) fn handle_provider_set(&self, params: Value) -> Result<Value, AcpError> {
        let request: SetProviderRequest = parse_params(params)?;
        let store = self.provider_store()?;
        store
            .upsert(providers::ProviderInput {
                id: request.provider.id,
                name: request.provider.name,
                base_url: request.provider.base_url,
                api_key: request.provider.api_key,
                models: request
                    .provider
                    .models
                    .into_iter()
                    .map(|model| providers::Model {
                        id: model.id,
                        name: model.name,
                        modalities: model.modalities,
                        context_window: model.context_window,
                        thinking: model.thinking,
                    })
                    .collect(),
            })
            .map_err(provider_rejected)?;
        Ok(serialize(SetProviderResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    pub(super) fn handle_provider_delete(&self, params: Value) -> Result<Value, AcpError> {
        let request: DeleteProviderRequest = parse_params(params)?;
        let store = self.provider_store()?;
        store
            .delete(&request.provider_id)
            .map_err(provider_rejected)?;
        Ok(serialize(DeleteProviderResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    pub(super) fn handle_provider_set_default(&self, params: Value) -> Result<Value, AcpError> {
        let request: SetDefaultProviderRequest = parse_params(params)?;
        let store = self.provider_store()?;
        store
            .set_default(&request.model)
            .map_err(provider_rejected)?;
        Ok(serialize(SetDefaultProviderResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }
}

/// The provider catalog as the wire reports it.
fn catalog(store: &ProviderStore) -> ProviderCatalog {
    ProviderCatalog {
        providers: store.views(),
        default_model: store.default_reference(),
    }
}

/// What a client is told when a provider change was refused.
///
/// A fault in the request is told apart from a fault in the machine: an id or a
/// URL the caller got wrong is theirs to fix, while a file that cannot be written
/// is not something they can do anything about.
fn provider_rejected(err: ProviderError) -> AcpError {
    let code = match &err {
        ProviderError::Invalid(_)
        | ProviderError::BadReference(_)
        | ProviderError::UnknownProvider(_)
        | ProviderError::UnknownModel(_) => INVALID_PARAMS,
        ProviderError::Config(_) | ProviderError::Io { .. } | ProviderError::Parse { .. } => {
            INTERNAL_ERROR
        }
    };
    AcpError::new(code, err.to_string())
}

#[cfg(test)]
mod tests {
    use crate::agent::support::*;

    use serde_json::json;
    use srud_core::client::ModelEvent;
    use srud_protocol::acp::methods::{
        SESSION_PROMPT, SRUD_PROVIDER_DELETE, SRUD_PROVIDER_LIST, SRUD_PROVIDER_SET,
        SRUD_PROVIDER_SET_DEFAULT,
    };
    use srud_protocol::error::INVALID_PARAMS;

    #[tokio::test]
    async fn a_provider_deleted_under_a_session_makes_its_turns_refuse() {
        let (agent, _factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;
        choose(&agent, &session_id, "anthropic/opus").await;

        // The provider goes away, as when a user removes it from the panel.
        result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_DELETE,
                    json!({ "providerId": "anthropic" }),
                ))
                .await,
        );

        // Refused rather than quietly run on another provider: a turn recorded
        // under one model while another answered would make the log a lie.
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hello" }],
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        assert!(
            error.message.contains("no longer configured"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn provider_methods_round_trip_over_rpc() {
        let (agent, _factory) = providers_agent(vec![]);
        initialize(&agent).await;

        let listed = result_of(agent.handle(request(SRUD_PROVIDER_LIST, json!({}))).await);
        assert_eq!(listed["providers"].as_array().unwrap().len(), 2);
        assert!(listed["default"].is_null(), "nothing is the default yet");
        // A key is readable back only as a mask — enough to tell two apart, not
        // enough to use one.
        assert_eq!(listed["providers"][0]["apiKey"], json!("sk-…-key"));
        assert_eq!(listed["providers"][0]["hasKey"], json!(true));

        let added = result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_SET,
                    json!({
                        "provider": {
                            "id": "local",
                            "name": "Local",
                            "baseUrl": "https://local.example/v1",
                            "apiKey": "sk-local-1234567890",
                            "models": [{ "id": "m", "name": "M" }],
                        }
                    }),
                ))
                .await,
        );
        assert_eq!(added["providers"].as_array().unwrap().len(), 3);

        let defaulted = result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_SET_DEFAULT,
                    json!({ "model": "openai/large" }),
                ))
                .await,
        );
        assert_eq!(defaulted["default"], json!("openai/large"));

        // And that is where the next session starts.
        let _home = home();
        assert_eq!(
            new_session_result(&agent).await["configOptions"][0]["currentValue"],
            json!("openai/large")
        );

        let removed = result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_DELETE,
                    json!({ "providerId": "local" }),
                ))
                .await,
        );
        assert_eq!(removed["providers"].as_array().unwrap().len(), 2);
    }
}
