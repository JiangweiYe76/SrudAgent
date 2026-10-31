//! The model a session runs on.
//!
//! A session names its model as a `providerId/modelId` reference and the client
//! is resolved per turn, so a switch takes effect on the next one. The choice
//! travels over ACP's own `configOptions`, which is also where a model's
//! thinking levels are offered; a switch re-projects the stored level onto the
//! new model's vocabulary.

use super::{parse_params, session_not_found, Agent};
use crate::providers::{self};
use serde_json::Value;
use srud_core::client::ModelClient;
use srud_protocol::acp::{
    AcpError, ConfigOptionUpdate, Notification, SessionConfigOption, SessionId,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, CLIENT_METHOD_NAMES,
};
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS, METHOD_NOT_FOUND};
use std::sync::Arc;

impl Agent {
    /// The model client a session runs on.
    ///
    /// Resolved per turn rather than held for the agent's life: the selection can
    /// change between two turns of one session, and a turn has to run on
    /// whichever model was chosen when it started.
    pub(super) fn client_for(&self, id: &SessionId) -> Result<Arc<dyn ModelClient>, AcpError> {
        let Some(providers) = &self.providers else {
            return self.fixed_client();
        };
        // A session that names no model is one created before any provider was
        // configured, or restored from a log written then. It runs on the client
        // the host supplied, which is what it ran on before — and when the host
        // supplied none, nothing is picked on the user's behalf. This is the
        // path a session takes when nobody has chosen its model yet, or when the
        // configured default names something that is no longer there. Saying so
        // is more use than running on a model nobody asked for.
        let Some(reference) = self.sessions.model(id) else {
            return self.fixed_client().map_err(|_| {
                AcpError::new(
                    INVALID_PARAMS,
                    "this session has no model yet; choose one from the model picker",
                )
            });
        };

        let (provider, model) = providers.store.resolve(&reference).map_err(|err| {
            // The session names a model that is no longer configured. Refused
            // rather than quietly run on something else: a turn recorded under
            // one model while another answered would make the log a lie about
            // what produced it.
            AcpError::new(
                INVALID_PARAMS,
                format!("this session's model is no longer configured: {err}"),
            )
        })?;
        providers
            .factory
            .build(
                &provider,
                &model,
                model.thinking_request(self.sessions.thinking(id)),
            )
            .map_err(|err| {
                AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot build a client for `{reference}`: {err}"),
                )
            })
    }

    /// The configured context window of the session's model.
    ///
    /// `None` when the host configured no providers, the session names no
    /// model, the model is no longer configured, or no window was set for it —
    /// in each case usage reports simply go unshown rather than guessed at.
    pub(super) fn context_size_for(&self, id: &SessionId) -> Option<u64> {
        let providers = self.providers.as_ref()?;
        let reference = self.sessions.model(id)?;
        let (_provider, model) = providers.store.resolve(&reference).ok()?;
        model.context_window
    }

    /// The client the host fixed at construction, when it fixed one.
    fn fixed_client(&self) -> Result<Arc<dyn ModelClient>, AcpError> {
        self.model.clone().ok_or_else(|| {
            AcpError::new(
                INTERNAL_ERROR,
                "the host supplied no model client and this session names no model",
            )
        })
    }

    /// The model selector, as this session should be offered it.
    ///
    /// `None` when the host configured no providers: an agent with one fixed
    /// client has nothing to choose between, and offering an empty selector
    /// would claim it had.
    pub(super) fn config_options(&self, id: &SessionId) -> Option<Vec<SessionConfigOption>> {
        let providers = self.providers.as_ref()?;
        providers.store.session_config_options(
            self.sessions.model(id).as_deref(),
            self.sessions.thinking(id).as_deref(),
        )
    }

    /// Chooses the model a session runs on.
    ///
    /// ACP's own method, not an extension: a client that knows nothing about
    /// SrudAgent still renders the selector and can still switch. That is also
    /// why the *global* default is not changed here — this request has a session
    /// id, and there is no way to say "and every other session too" in it.
    pub(super) async fn handle_set_config_option(
        &self,
        params: Value,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        let request: SetSessionConfigOptionRequest = parse_params(params)?;
        let Some(providers) = &self.providers else {
            return Err(AcpError::new(
                METHOD_NOT_FOUND,
                "this agent's model is not configurable",
            ));
        };
        match request.config_id.0.as_ref() {
            providers::MODEL_OPTION_ID => {
                let reference = request
                    .value
                    .as_value_id()
                    .ok_or_else(|| {
                        AcpError::new(INVALID_PARAMS, "the model option takes a string value")
                    })?
                    .0
                    .to_string();

                // Resolved before it is recorded. A reference nothing resolves to
                // would leave the session unable to take a turn, while its log
                // claimed the choice was deliberate.
                let (_, model) = providers
                    .store
                    .resolve(&reference)
                    .map_err(|err| AcpError::new(INVALID_PARAMS, err.to_string()))?;

                match self
                    .sessions
                    .set_model(&request.session_id, Some(reference))
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => return Err(session_not_found(&request.session_id)),
                    Err(err) => {
                        return Err(AcpError::new(
                            INTERNAL_ERROR,
                            format!("cannot record the model: {err}"),
                        ));
                    }
                }

                // The level is stored model-agnostically, so a switch can land
                // on a model that does not offer the old one. Project it now, so
                // the selector names the level the next turn will send.
                self.reproject_thinking(&request.session_id, &model).await?;
            }
            providers::THINKING_OPTION_ID => {
                let value = request
                    .value
                    .as_value_id()
                    .ok_or_else(|| {
                        AcpError::new(INVALID_PARAMS, "the thinking option takes a string value")
                    })?
                    .0
                    .to_string();
                let level = value
                    .parse::<srud_protocol::srud::methods::ThinkingLevel>()
                    .map_err(|err| AcpError::new(INVALID_PARAMS, err))?;

                let reference = self.sessions.model(&request.session_id).ok_or_else(|| {
                    AcpError::new(
                        INVALID_PARAMS,
                        "this session has no model yet; choose one first",
                    )
                })?;
                let (_, model) = providers
                    .store
                    .resolve(&reference)
                    .map_err(|err| AcpError::new(INVALID_PARAMS, err.to_string()))?;
                let offered = model
                    .thinking
                    .as_ref()
                    .is_some_and(|thinking| thinking.levels.contains_key(&level));
                if !offered {
                    return Err(AcpError::new(
                        INVALID_PARAMS,
                        format!(
                            "`{}` is not a thinking level `{reference}` offers",
                            level.as_str()
                        ),
                    ));
                }

                match self
                    .sessions
                    .set_thinking(&request.session_id, Some(level.as_str().to_owned()))
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => return Err(session_not_found(&request.session_id)),
                    Err(err) => {
                        return Err(AcpError::new(
                            INTERNAL_ERROR,
                            format!("cannot record the thinking level: {err}"),
                        ));
                    }
                }
            }
            other => {
                return Err(AcpError::new(
                    INVALID_PARAMS,
                    format!("no configuration option `{other}`"),
                ));
            }
        }

        // The response carries the whole set rather than an acknowledgement, so
        // a client renders the result of its own change without asking again.
        match self.config_options(&request.session_id) {
            Some(options) => {
                self.broadcast_config_options(&request.session_id, options.clone());
                Ok(SetSessionConfigOptionResponse::new(options))
            }
            None => Ok(SetSessionConfigOptionResponse::new(Vec::new())),
        }
    }

    /// Records the level this session will actually send under `model`.
    ///
    /// A level is stored in the model-agnostic vocabulary, so a model switch can
    /// leave it unsupported. For a model that declares thinking the level is
    /// projected down to the nearest one it offers; for a model that declares
    /// none the stored level is left alone, so switching back restores the
    /// user's choice instead of silently resetting it. Nothing is written when
    /// the projection is the value already stored.
    async fn reproject_thinking(
        &self,
        id: &SessionId,
        model: &providers::Model,
    ) -> Result<(), AcpError> {
        if model.thinking.is_none() {
            return Ok(());
        }
        let current = self.sessions.thinking(id);
        let next = model
            .thinking_level(current.as_deref())
            .map(|level| level.as_str().to_owned());
        if current == next {
            return Ok(());
        }
        match self.sessions.set_thinking(id, next).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(session_not_found(id)),
            Err(err) => Err(AcpError::new(
                INTERNAL_ERROR,
                format!("cannot record the thinking level: {err}"),
            )),
        }
    }

    /// Publishes a configuration change on the `session/update` stream.
    ///
    /// The same channel `session_info_update` travels on, so a client already
    /// following `session/update` sees a model switched by another client
    /// without having asked.
    fn broadcast_config_options(&self, session_id: &SessionId, options: Vec<SessionConfigOption>) {
        self.hub.send(Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
            )),
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::support::*;
    use crate::providers::{Model, ProviderInput};

    use serde_json::json;
    use srud_core::client::ModelEvent;
    use srud_protocol::acp::methods::{
        SESSION_PROMPT, SESSION_SET_CONFIG_OPTION, SRUD_PROVIDER_SET_DEFAULT,
    };
    use srud_protocol::acp::SessionId;
    use srud_protocol::error::{INVALID_PARAMS, METHOD_NOT_FOUND};

    #[tokio::test]
    async fn a_new_session_offers_the_configured_models() {
        let (agent, _factory) = providers_agent(vec![]);
        initialize(&agent).await;
        let _home = home();

        let result = new_session_result(&agent).await;
        let options = result["configOptions"]
            .as_array()
            .expect("a model selector");
        assert_eq!(options.len(), 1, "the model option is the only one offered");
        let option = &options[0];
        assert_eq!(option["id"], json!("model"));
        assert_eq!(option["category"], json!("model"));
        assert_eq!(option["type"], json!("select"));
        // Nobody has chosen a default, so nothing is chosen for the session
        // either. The empty value is what the picker renders as "choose one".
        assert_eq!(option["currentValue"], json!(""));

        let groups = option["options"].as_array().expect("grouped by provider");
        let named: Vec<&str> = groups
            .iter()
            .map(|group| group["name"].as_str().unwrap())
            .collect();
        assert_eq!(named, ["Provider openai", "Provider anthropic"]);
        assert_eq!(groups[0]["group"], json!("openai"));
        let values: Vec<&str> = groups[0]["options"]
            .as_array()
            .expect("a group's options")
            .iter()
            .map(|value| value["value"].as_str().unwrap())
            .collect();
        assert_eq!(values, ["openai/small", "openai/large"]);
    }

    #[tokio::test]
    async fn a_new_session_starts_on_the_chosen_default() {
        let (agent, factory) = providers_agent(vec![]);
        initialize(&agent).await;
        let _home = home();
        // Chosen through the panel's own method, which is the only writer of it.
        result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_SET_DEFAULT,
                    json!({ "model": "anthropic/opus" }),
                ))
                .await,
        );

        let result = new_session_result(&agent).await;
        assert_eq!(
            result["configOptions"][0]["currentValue"],
            json!("anthropic/opus")
        );

        let session_id = SessionId::new(result["sessionId"].as_str().unwrap());
        prompt_once(&agent, &session_id).await;
        assert_eq!(factory.built(), vec!["anthropic/opus"]);
    }

    #[tokio::test]
    async fn a_session_with_no_model_and_no_fallback_is_told_to_choose_one() {
        let (agent, factory) = providers_agent_with(vec![], false);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

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
            error.message.contains("choose one"),
            "the message says what to do: {}",
            error.message
        );
        assert!(factory.built().is_empty(), "nothing ran");
    }

    #[tokio::test]
    async fn a_session_with_no_model_runs_on_the_client_the_host_fixed() {
        // A log written before the provider file existed names no model. It runs
        // on what it ran on then rather than on a model chosen since.
        let (agent, factory) =
            providers_agent_with(vec![ModelEvent::TextDelta { delta: "ok".into() }], true);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        prompt_once(&agent, &session_id).await;
        assert!(factory.built().is_empty(), "no provider was asked for one");
    }

    #[tokio::test]
    async fn a_chosen_model_runs_the_turn_and_is_broadcast() {
        let (agent, factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        let mut rx = agent.subscribe();
        let result = choose(&agent, &session_id, "anthropic/opus").await;
        // The reply carries the whole set, so the client renders its own change.
        assert_eq!(
            result["configOptions"][0]["currentValue"],
            json!("anthropic/opus")
        );

        let update = serde_json::to_value(rx.try_recv().expect("the change is announced")).unwrap();
        assert_eq!(
            update["params"]["update"]["sessionUpdate"],
            json!("config_option_update")
        );
        assert_eq!(
            update["params"]["update"]["configOptions"][0]["currentValue"],
            json!("anthropic/opus")
        );
        assert_eq!(update["params"]["sessionId"], json!(session_id.0.as_ref()));

        prompt_once(&agent, &session_id).await;
        assert_eq!(factory.built(), vec!["anthropic/opus"]);
    }

    #[tokio::test]
    async fn a_thinking_level_is_offered_chosen_and_kept_across_models() {
        use srud_protocol::srud::methods::{ThinkingConfig, ThinkingLevel};

        let (agent, _factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        agent
            .provider_store()
            .expect("a provider store")
            .upsert(ProviderInput {
                id: "thinking".into(),
                name: "Thinking".into(),
                base_url: "https://thinking.example/v1".into(),
                api_key: Some("sk-thinking".into()),
                models: vec![Model {
                    id: "m".into(),
                    name: None,
                    modalities: Vec::new(),
                    context_window: None,
                    thinking: Some(ThinkingConfig {
                        default: ThinkingLevel::High,
                        levels: [
                            (ThinkingLevel::Low, "low".to_owned()),
                            (ThinkingLevel::Medium, "medium".to_owned()),
                            (ThinkingLevel::High, "high".to_owned()),
                        ]
                        .into_iter()
                        .collect(),
                    }),
                }],
            })
            .expect("a valid provider");

        let session_id = new_session_in(&agent, "").await;
        let result = choose(&agent, &session_id, "thinking/m").await;
        assert_eq!(result["configOptions"][1]["id"], json!("reasoning"));
        assert_eq!(result["configOptions"][1]["currentValue"], json!("high"));

        let result = result_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "reasoning",
                        "value": "low",
                    }),
                ))
                .await,
        );
        assert_eq!(result["configOptions"][1]["currentValue"], json!("low"));

        // A model that declares no thinking offers no level, and the stored
        // choice survives the switch so switching back restores it.
        let result = choose(&agent, &session_id, "openai/small").await;
        assert_eq!(
            result["configOptions"].as_array().expect("options").len(),
            1
        );
        let result = choose(&agent, &session_id, "thinking/m").await;
        assert_eq!(result["configOptions"][1]["currentValue"], json!("low"));

        // A level the model does not declare is refused rather than recorded.
        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "reasoning",
                        "value": "extreme",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn an_unknown_model_is_refused_and_the_session_keeps_its_own() {
        let (agent, factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;
        // Chosen first, so there is an "own" for the refusal to leave alone.
        result_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "model",
                        "value": "openai/small",
                    }),
                ))
                .await,
        );

        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "model",
                        "value": "openai/missing",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);

        // Refused, so the session still runs on what it was seeded with.
        prompt_once(&agent, &session_id).await;
        assert_eq!(factory.built(), vec!["openai/small"]);
    }

    #[tokio::test]
    async fn setting_an_option_that_does_not_exist_is_invalid_params() {
        let (agent, _factory) = providers_agent(vec![]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "mode",
                        "value": "anthropic/opus",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn an_agent_without_providers_offers_no_model_option() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let _home = home();

        let result = new_session_result(&agent).await;
        assert!(
            result["configOptions"].is_null(),
            "a fixed client has nothing to choose between"
        );

        // The method is not merely unpopulated — it is refused outright, matching
        // what the capabilities promised.
        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": result["sessionId"].as_str().unwrap(),
                        "configId": "model",
                        "value": "openai/small",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), METHOD_NOT_FOUND);
    }
}
