//! Bringing a session back from its log.
//!
//! `session/load` rebuilds the session and replays its conversation as
//! `session/update` notifications before the response, because ACP requires the
//! client to hold the whole conversation by the time the call resolves.
//! `session/resume` rebuilds it without replaying. How each recorded turn ended
//! rides on the `session/load` response: the replayed stream carries no turn
//! boundaries.

use super::{parse_params, session_not_found, Agent};
use crate::convert::{end_reason_wire, replay_notification};
use crate::sessions::RestoreError;
use serde_json::Value;
use srud_protocol::acp::{
    AcpError, LoadSessionRequest, LoadSessionResponse, ResumeSessionRequest, ResumeSessionResponse,
    SessionId,
};
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS};
use srud_protocol::srud::meta::{SrudMeta, SrudTurnEnd};

impl Agent {
    pub(super) async fn handle_resume(
        &self,
        params: Value,
    ) -> Result<ResumeSessionResponse, AcpError> {
        let request: ResumeSessionRequest = parse_params(params)?;
        let loaded = self.restore(&request.session_id, &request.cwd).await?;
        // No replay: a client asking to resume is saying it already has the
        // conversation.
        self.broadcast_title(&request.session_id, loaded.title.clone());
        let mut response = ResumeSessionResponse::new();
        if let Some(options) = self.config_options(&request.session_id) {
            response = response.config_options(options);
        }
        Ok(response)
    }

    pub(super) async fn handle_load(&self, params: Value) -> Result<LoadSessionResponse, AcpError> {
        let request: LoadSessionRequest = parse_params(params)?;
        let loaded = self.restore(&request.session_id, &request.cwd).await?;

        // Replayed before the response, because ACP requires the client to have
        // the whole conversation by the time `session/load` resolves. Anything
        // sent after would arrive as a change to a session the client already
        // believes it has.
        let context_size = self.context_size_for(&request.session_id);
        for event in &loaded.conversation {
            if let Some(notification) =
                replay_notification(&request.session_id, event, context_size)
            {
                self.hub.send(notification);
            }
        }
        self.broadcast_title(&request.session_id, loaded.title.clone());
        let mut response = LoadSessionResponse::new();
        // ACP has this response carry the options, so a client being shown a
        // session is told which model it ran on without asking.
        if let Some(options) = self.config_options(&request.session_id) {
            response = response.config_options(options);
        }
        // How each turn ended travels here: a replayed item is the update live
        // streaming sent, and none of those carries a turn boundary. A turn the log
        // left open is absent from the list, which a client reads as still running.
        let turns: Vec<SrudTurnEnd> = loaded
            .turn_ends
            .iter()
            .map(|(turn_id, reason)| SrudTurnEnd {
                turn_id: turn_id.to_string(),
                end_reason: end_reason_wire(*reason).to_string(),
            })
            .collect();
        let srud = SrudMeta {
            turns: Some(turns),
            ..Default::default()
        };
        Ok(response.meta(srud.to_meta()))
    }

    /// Brings a session back, or says why it could not be.
    async fn restore(
        &self,
        id: &SessionId,
        cwd: &std::path::Path,
    ) -> Result<srud_core::session_store::Loaded, AcpError> {
        self.sessions.restore(id, cwd).await.map_err(restore_failed)
    }
}

/// What a client is told when a session could not be brought back.
fn restore_failed(err: RestoreError) -> AcpError {
    match &err {
        RestoreError::NotFound(id) => session_not_found(id),
        // The caller named a different directory, which ACP requires to match:
        // that is a bad request, not a server fault.
        RestoreError::Cwd { .. } => AcpError::new(INVALID_PARAMS, err.to_string()),
        RestoreError::Config(_) | RestoreError::Load(_) | RestoreError::Repair(_) => {
            AcpError::new(INTERNAL_ERROR, format!("cannot load the session: {err}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::support::*;
    use crate::agent::*;
    use crate::config;
    use crate::providers::ProviderStore;
    use srud_core::tools::ToolRegistry;
    use std::sync::Arc;

    use crate::test_env::{self, Guard};
    use serde_json::json;
    use srud_core::client::{ModelEvent, TokenUsage};
    use srud_protocol::acp::methods::{SESSION_LOAD, SESSION_RESUME};
    use srud_protocol::error::{INVALID_PARAMS, SESSION_NOT_FOUND};

    #[tokio::test]
    async fn a_session_loads_back_from_its_log() {
        // The point of the log: an agent that restarted has nothing in memory and
        // everything on disk.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        // A second agent, as after a restart.
        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx),
            vec![
                ("user_message_chunk".to_string(), "hello".to_string()),
                ("agent_message_chunk".to_string(), "hi there".to_string()),
            ],
            "the conversation is replayed in order"
        );
    }

    #[tokio::test]
    async fn a_loaded_session_continues_where_it_left_off() {
        // Loading is not just replay: the agent has to have the history too, or
        // the next prompt answers as though the session were new.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-continue"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "first answer".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "first question").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        let session = second
            .sessions
            .get(&session_id)
            .expect("the session is live");
        assert_eq!(
            session.state().history(),
            &[
                // The env-context block the first turn recorded: it is part of
                // what the model saw, so it comes back with it.
                srud_core::context::item(std::path::Path::new(&cwd)),
                srud_core::types::ResponseItem::user("first question"),
                srud_core::types::ResponseItem::assistant("first answer"),
            ]
        );
    }

    #[tokio::test]
    async fn resuming_does_not_replay_the_conversation() {
        // What a client asks for by resuming: it already has the transcript.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-resume"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_RESUME,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert!(conversation(&mut rx).is_empty(), "nothing is replayed");
        assert!(second.sessions.get(&session_id).is_some(), "and it is live");
    }

    #[tokio::test]
    async fn a_live_session_loads_for_another_client() {
        // One client has the session open and another asks for it: the
        // conversation comes off the log, and nothing is invented for a session
        // that never stopped.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-live"),
        );

        let agent = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&agent).await;
        let (session_id, cwd) = session_with_a_home(&agent).await;
        prompt_titles(&agent, &session_id, "hello").await;

        let mut rx = agent.subscribe();
        result_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx),
            vec![
                ("user_message_chunk".to_string(), "hello".to_string()),
                ("agent_message_chunk".to_string(), "hi there".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn loading_a_session_that_never_existed_is_not_found() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-missing"),
        );

        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": "00000000-0000-4000-8000-000000000000",
                        "cwd": std::env::temp_dir().to_string_lossy(),
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn loading_in_another_directory_is_refused() {
        // ACP requires the caller's cwd to match the session's, and it has to:
        // the history was produced somewhere.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-cwd-mismatch"),
        );

        let first = agent(scripted(vec![]));
        initialize(&first).await;
        let (session_id, _cwd) = session_with_a_home(&first).await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let error = error_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": std::env::temp_dir().to_string_lossy(),
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn loading_without_naming_a_directory_is_allowed() {
        // A client that named no directory is not disagreeing with the recorded
        // one, so it is not refused here: the app has no directory picker and says
        // so on load the same way it does when it creates a session. Refusing would
        // leave every session from a previous run unloadable, which after a restart
        // is all of them.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-cwd-absent"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, _cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": "",
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx).len(),
            2,
            "the conversation comes back, not an empty session"
        );
    }

    #[tokio::test]
    async fn the_load_response_says_how_each_turn_ended() {
        // A client being shown a session has no `session/prompt` response to close
        // its turns with, so the response to `session/load` lists them.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-turn-ends"),
        );

        // Each turn reports usage, so each one's log ends with a usage record.
        let first = agent(scripted(vec![
            ModelEvent::TextDelta {
                delta: "answer".into(),
            },
            ModelEvent::Usage(TokenUsage {
                input_tokens: 12,
                output_tokens: 3,
                ..Default::default()
            }),
        ]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;
        prompt_titles(&first, &session_id, "again").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        let response = result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        let turns = response["_meta"]["srud"]["turns"]
            .as_array()
            .expect("the load response lists the turns");
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_ne!(
            turns[0]["turnId"], turns[1]["turnId"],
            "each turn is its own"
        );
        for turn in turns {
            assert_eq!(turn["endReason"], json!("completed"), "{turn:?}");
        }

        // A replayed update is the update live streaming sent. The usage records
        // are not among them: this agent configures no context window.
        let updates = replayed_updates(&mut rx);
        assert_eq!(updates.len(), 4, "{updates:?}");
        assert!(
            updates.iter().all(|(_, end)| end.is_none()),
            "no update closes a turn: {updates:?}"
        );
    }

    #[tokio::test]
    async fn a_restored_session_remembers_the_model_it_chose() {
        let _env = Guard::take(&[config::HOME_VAR]);
        std::env::set_var(config::HOME_VAR, test_env::unique_dir("srud-agent-restore"));
        let store = Arc::new(ProviderStore::load().expect("an empty store"));
        store
            .upsert(sample_provider("openai", &["small", "large"]))
            .unwrap();
        store
            .upsert(sample_provider("anthropic", &["opus"]))
            .unwrap();

        let factory = RecordingFactory::new(vec![ModelEvent::TextDelta { delta: "hi".into() }]);
        let first = Agent::with_providers(
            Some(scripted(Vec::new())),
            Arc::new(ToolRegistry::new()),
            ModelProviders::new(store.clone(), factory),
        );
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        choose(&first, &session_id, "anthropic/opus").await;
        prompt_once(&first, &session_id).await;
        drop(first);

        // A second agent, as after a restart, rebuilt from the same log.
        let second = Agent::with_providers(
            Some(scripted(Vec::new())),
            Arc::new(ToolRegistry::new()),
            ModelProviders::new(store, RecordingFactory::new(Vec::new())),
        );
        initialize(&second).await;
        let result = result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );
        assert_eq!(
            result["configOptions"][0]["currentValue"],
            json!("anthropic/opus"),
            "the choice survives the restart"
        );
    }
}
