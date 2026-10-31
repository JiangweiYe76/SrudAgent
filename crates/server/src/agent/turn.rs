//! Running a turn.
//!
//! `session/prompt` runs one turn and resolves when it ends — the response is
//! the turn's termination signal. `session/cancel` interrupts it cooperatively,
//! `_srud/unstable/session/steer` injects input into a turn already in flight,
//! and `_srud/unstable/session/regenerate` takes the last turn's answer back and
//! answers the same words again.

use super::{parse_params, prompt_parts, session_not_found, Agent};
use crate::convert::prompt_outcome;
use serde_json::Value;
use srud_core::{EventSink, SessionEvent, SessionWriter};
use srud_protocol::acp::{AcpError, CancelNotification, PromptRequest, PromptResponse, SessionId};
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS, SESSION_BUSY};
use srud_protocol::srud::methods::{
    RegenerateRequest, RegenerateResponse, SteerRequest, SteerResponse,
};
use srud_protocol::srud::turn_end::TurnEndWire;
use std::sync::atomic::Ordering;
use std::time::Duration;

impl Agent {
    pub(super) async fn handle_prompt(&self, params: Value) -> Result<PromptResponse, AcpError> {
        // Held for the whole turn, so `shutdown`'s probe succeeds only once the
        // turn has settled.
        let _running = self.prompt_gate.read().await;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AcpError::new(INTERNAL_ERROR, "the agent is shutting down"));
        }

        let request: PromptRequest = parse_params(params)?;

        // The session and its log together, so a turn cannot be recorded against
        // another session's log. One lookup rather than two: the pair is
        // registered together and is never separated.
        let (session, log) = self
            .sessions
            .with_log(&request.session_id)
            .ok_or_else(|| session_not_found(&request.session_id))?;

        // Resolved before anything is named or recorded: a prompt with no model
        // to run on is not a turn, and a session named by one would be named
        // after a message that never ran.
        let client = self.client_for(&request.session_id)?;

        let parts = prompt_parts(&request.prompt)?;

        let sink = self
            .hub
            .sink_for(session.id(), self.context_size_for(&request.session_id));
        // Registering the turn's sink is the busy gate: steer announcements reach
        // the running turn through this map, so a second prompt must neither
        // overwrite the live sink nor remove it when it unwinds. The gate also
        // precedes naming, so a refused prompt leaves no trace.
        {
            let mut sinks = self
                .active_sinks
                .lock()
                .expect("active sinks lock poisoned");
            if sinks.contains_key(&request.session_id) {
                return Err(AcpError::new(
                    SESSION_BUSY,
                    "session already has an active turn",
                ));
            }
            sinks.insert(request.session_id.clone(), sink.clone());
        }

        self.name_from_first_prompt(&request.session_id, &parts.text)
            .await;

        let result = srud_core::turn::run_turn(
            &session,
            srud_core::types::TurnInput {
                text: parts.text,
                images: parts.images,
            },
            client.as_ref(),
            self.tools.as_ref(),
            log.as_ref(),
            &sink,
        )
        .await;
        self.active_sinks
            .lock()
            .expect("active sinks lock poisoned")
            .remove(&request.session_id);
        let result = result.map_err(|err| match err {
            srud_core::TurnError::Busy => {
                AcpError::new(SESSION_BUSY, "session already has an active turn")
            }
            // Distinct from `Busy`: nothing ran, and the session is still usable —
            // the log is what refused. Saying "busy" would tell the client to wait
            // for something that will never start.
            srud_core::TurnError::Unrecordable(err) => {
                AcpError::new(INTERNAL_ERROR, format!("cannot record this turn: {err}"))
            }
        })?;

        let wire: TurnEndWire = prompt_outcome(result.reason)?;
        let response = PromptResponse::new(wire.stop_reason);
        Ok(match wire.meta {
            Some(meta) => response.meta(meta.to_meta()),
            None => response,
        })
    }

    pub(super) fn handle_cancel(&self, params: Value) -> Result<(), AcpError> {
        let request: CancelNotification = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        // Cooperative: this signals the turn's token; the prompt reply then
        // carries `stopReason: "cancelled"`.
        session.interrupt();
        Ok(())
    }

    /// Interrupts a running turn and waits for it to close.
    ///
    /// The prompt that owns the turn removes its sink only after the turn has
    /// ended, so waiting for that removal is how this learns the turn is over
    /// without holding anything the prompt holds.
    ///
    /// Returns `false` on giving up, which only happens when the prompt never
    /// settles. The caller then refuses rather than starting a second turn on a
    /// session that still has one: two turns would record into one history in
    /// whatever order they happened to finish.
    async fn interrupt_and_wait(&self, session_id: &SessionId) -> bool {
        // Loop, because a turn that had not claimed the session yet when the
        // first signal arrived claims it just after — the same reason
        // `shutdown` re-signals while it drains.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        while self
            .active_sinks
            .lock()
            .expect("active sinks lock poisoned")
            .contains_key(session_id)
        {
            if let Some(session) = self.sessions.get(session_id) {
                session.interrupt();
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        true
    }

    /// Answers the last turn again, after discarding what it said.
    ///
    /// The session's history is the only thing that decides what the model sees,
    /// so the answer is taken back there before the second request is built —
    /// not merely stopped on the client. What the discard removes is everything
    /// the last turn appended, and nothing before it: the message the turn
    /// answered stays, which is what lets the replacement answer the same words
    /// without them being recorded twice.
    ///
    /// A turn still running is interrupted first and its end awaited, because a
    /// turn being generated is not a turn whose output can be taken back yet.
    pub(super) async fn handle_regenerate(
        &self,
        params: Value,
    ) -> Result<RegenerateResponse, AcpError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AcpError::new(INTERNAL_ERROR, "the agent is shutting down"));
        }

        let request: RegenerateRequest = parse_params(params)?;
        let (session, log) = self
            .sessions
            .with_log(&request.session_id)
            .ok_or_else(|| session_not_found(&request.session_id))?;
        let client = self.client_for(&request.session_id)?;
        let opened = session.last_prompt().ok_or_else(|| {
            AcpError::new(
                INVALID_PARAMS,
                "only a turn this session ran can be answered again",
            )
        })?;

        // A turn still arriving is stopped first and then replaced: a user
        // pressing regenerate while an answer is streaming means "not this one",
        // and the partial answer is as discardable as a finished one.
        if !self.interrupt_and_wait(&request.session_id).await {
            return Err(AcpError::new(
                SESSION_BUSY,
                "the running turn did not stop; try again once it has",
            ));
        }

        // Taken under the same lock the prompt's busy gate uses, so a prompt
        // that started between the wait and here cannot have its output cut.
        let sink = self
            .hub
            .sink_for(session.id(), self.context_size_for(&request.session_id));
        {
            let mut sinks = self
                .active_sinks
                .lock()
                .expect("active sinks lock poisoned");
            if sinks.contains_key(&request.session_id) {
                return Err(AcpError::new(
                    SESSION_BUSY,
                    "session already has an active turn",
                ));
            }
            sinks.insert(request.session_id.clone(), sink.clone());
        }

        // The replacement is a turn, so it holds the gate `shutdown` waits on for
        // the whole of its run.
        let _running = self.prompt_gate.read().await;

        // Only the latest turn can be taken back: an older one has turns written
        // after it, and cutting history at its answer would take them with it.
        let latest = log.last_turn().await.unwrap_or_default();
        if latest.map(|id| id.to_string()).as_deref() != Some(request.turn_id.as_str()) {
            self.active_sinks
                .lock()
                .expect("active sinks lock poisoned")
                .remove(&request.session_id);
            return Err(AcpError::new(
                INVALID_PARAMS,
                "only the session's latest turn can be answered again",
            ));
        }
        let latest = latest.expect("checked above");

        // Recorded before the history is cut and before anything of the
        // replacement goes out: a session that could not record the discard
        // still has the answer it had, rather than a memory the log disagrees
        // with. A turn whose entries the client was shown but the log never
        // wrote has nothing to record — history already holds nothing of it.
        let cut = session.state().len() > opened.mark;
        if cut {
            if let Err(err) =
                SessionWriter::record(log.as_ref(), &SessionEvent::turn_discarded(latest)).await
            {
                self.active_sinks
                    .lock()
                    .expect("active sinks lock poisoned")
                    .remove(&request.session_id);
                return Err(AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot record this discard: {err}"),
                ));
            }
            session.truncate(opened.mark);
        }
        // Always announced, discarded or not: a client showing the turn must
        // drop it, and a client showing nothing is unaffected. Sent under the
        // discarded turn's own id rather than held until the replacement names
        // itself, because it is the answer that is leaving, not the one starting.
        sink.broadcast(latest, srud_core::Event::TurnDiscarded { turn_id: latest });

        let result = srud_core::run_turn_after(
            &session,
            opened.input,
            Some(opened.mark),
            client.as_ref(),
            self.tools.as_ref(),
            log.as_ref(),
            &sink,
        )
        .await;
        self.active_sinks
            .lock()
            .expect("active sinks lock poisoned")
            .remove(&request.session_id);
        let result = result.map_err(|err| match err {
            srud_core::TurnError::Busy => {
                AcpError::new(SESSION_BUSY, "session already has an active turn")
            }
            srud_core::TurnError::Unrecordable(err) => {
                AcpError::new(INTERNAL_ERROR, format!("cannot record this turn: {err}"))
            }
        })?;

        let wire: TurnEndWire = prompt_outcome(result.reason)?;
        Ok(match wire.meta {
            Some(meta) => RegenerateResponse {
                stop_reason: wire.stop_reason,
                meta: meta.to_meta(),
            },
            None => RegenerateResponse {
                stop_reason: wire.stop_reason,
                meta: None,
            },
        })
    }

    pub(super) async fn handle_steer(&self, params: Value) -> Result<SteerResponse, AcpError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AcpError::new(INTERNAL_ERROR, "the agent is shutting down"));
        }

        let request: SteerRequest = parse_params(params)?;
        let session = self
            .sessions
            .get(&request.session_id)
            .ok_or_else(|| session_not_found(&request.session_id))?;
        let parts = prompt_parts(&request.prompt)?;
        if !parts.images.is_empty() {
            return Err(AcpError::new(
                INVALID_PARAMS,
                "steering accepts text prompts only",
            ));
        }
        if !session.steer(&parts.text) {
            return Err(AcpError::new(
                SESSION_BUSY,
                "session has no active turn to steer",
            ));
        }
        // Announce the injected message the way a normal prompt does.
        if let Some(sink) = self
            .active_sinks
            .lock()
            .expect("active sinks lock poisoned")
            .get(&request.session_id)
            .cloned()
        {
            sink.emit(srud_core::types::Event::UserMessage {
                content: parts.text.clone(),
            });
        }
        Ok(SteerResponse::default())
    }
}

#[cfg(test)]
mod tests {
    use crate::agent::support::*;
    use crate::agent::*;
    use std::sync::Arc;

    use serde_json::json;
    use srud_core::client::ModelEvent;
    use srud_protocol::acp::methods::{
        SESSION_CANCEL, SESSION_LOAD, SESSION_PROMPT, SRUD_SESSION_REGENERATE,
    };
    use srud_protocol::acp::{ContentBlock, Response, TextContent};
    use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS, SESSION_BUSY, SESSION_NOT_FOUND};

    #[tokio::test]
    async fn prompt_streams_updates_and_returns_stop_reason() {
        let agent = agent(scripted(vec![
            ModelEvent::TextDelta { delta: "he".into() },
            ModelEvent::TextDelta {
                delta: "llo".into(),
            },
        ]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let mut rx = agent.subscribe();

        let result = result_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hi" }]
                    }),
                ))
                .await,
        );
        assert_eq!(result["stopReason"], json!("end_turn"));

        let mut kinds = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            assert_eq!(value["params"]["sessionId"], json!(session_id.0.as_ref()));
            let kind = value["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap()
                .to_string();
            // Session-metadata updates are not turn output, so they carry no
            // turn id; everything the turn loop emits does.
            if kind != "session_info_update" {
                assert!(
                    value["params"]["_meta"]["srud"]["turnId"].is_string(),
                    "{kind} carries a turn id"
                );
            }
            kinds.push(kind);
        }
        assert_eq!(
            kinds,
            vec![
                // The first prompt names the session, before any turn output.
                "session_info_update",
                "user_message_chunk",
                "agent_message_chunk",
                "agent_message_chunk"
            ]
        );
    }

    #[tokio::test]
    async fn cancel_makes_prompt_return_cancelled() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let prompt = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        // Give the turn a moment to claim the slot and emit its first delta.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cancel_reply = agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        assert!(matches!(cancel_reply.inner(), Response::Result { .. }));

        let result = result_of(prompt.await.unwrap());
        assert_eq!(result["stopReason"], json!("cancelled"));
    }

    #[tokio::test]
    async fn shutdown_interrupts_an_active_turn_and_refuses_new_prompts() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let prompt = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        for _ in 0..100 {
            if agent
                .sessions
                .get(&session_id)
                .is_some_and(|session| session.is_busy())
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            agent
                .sessions
                .get(&session_id)
                .is_some_and(|session| session.is_busy()),
            "the turn should have claimed the session before shutdown"
        );

        assert!(
            agent.shutdown(std::time::Duration::from_secs(2)).await,
            "the in-flight turn should settle within the timeout"
        );
        let result = result_of(prompt.await.unwrap());
        assert_eq!(result["stopReason"], json!("cancelled"));

        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "after" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INTERNAL_ERROR);
        assert!(
            error.message.contains("shutting down"),
            "the refusal names why: {error:?}"
        );
    }

    #[tokio::test]
    async fn prompt_again_while_busy_is_session_busy() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let first = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "second" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_BUSY);

        // Clean up: cancel so the in-flight turn finishes.
        agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        let _ = first.await.unwrap();
    }

    #[tokio::test]
    async fn prompt_on_unknown_session_is_session_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": "missing",
                        "prompt": [{ "type": "text", "text": "hi" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn audio_prompt_is_invalid_params() {
        let agent = agent(scripted(vec![ModelEvent::TextDelta { delta: "ok".into() }]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Audio(srud_protocol::acp::AudioContent::new("data", "audio/wav")),
        ];
        let params = json!({
            "sessionId": session_id.0.as_ref(),
            "prompt": serde_json::to_value(&blocks).unwrap(),
        });
        let error = error_of(agent.handle(request(SESSION_PROMPT, params)).await);
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn a_regenerate_answers_the_last_turn_again() {
        let agent = agent(scripted(vec![
            ModelEvent::TextDelta {
                delta: "first answer".into(),
            },
            ModelEvent::Done,
        ]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let turn_id = turn_id_of(&agent, &session_id, "Hola").await;
        assert!(!turn_id.is_empty(), "the prompt named its turn");

        let mut rx = agent.subscribe();
        let reply = tokio::time::timeout(
            Duration::from_secs(10),
            agent.handle(request(
                SRUD_SESSION_REGENERATE,
                json!({ "sessionId": session_id.0.as_ref(), "turnId": turn_id }),
            )),
        )
        .await
        .expect("a regenerate resolves rather than hanging");
        let value = result_of(reply);
        assert_eq!(value["stopReason"], json!("end_turn"), "{value:?}");

        // The question is not said again: the discard takes the answer, and the
        // replacement answers words the history already holds.
        let mut asked = 0;
        let mut discarded: Option<String> = None;
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            let update = &value["params"]["update"];
            if update["sessionUpdate"] == json!("user_message_chunk") {
                asked += 1;
            }
            if let Some(id) = value["params"]["_meta"]["srud"]["turnDiscarded"].as_str() {
                discarded = Some(id.to_string());
            }
        }
        assert_eq!(
            discarded.as_deref(),
            Some(turn_id.as_str()),
            "the discard names the turn it takes back, not the one replacing it"
        );
        assert_eq!(asked, 0, "the question is not said again");
    }

    #[tokio::test]
    async fn a_restored_session_can_answer_its_last_turn_again() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-regenerate"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "first answer".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "Hola").await;
        drop(first);

        // A second agent, as after a restart: nothing in memory, everything in
        // the log. The question it must answer again is read back from history.
        let second = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "second answer".into(),
        }]));
        initialize(&second).await;
        let loaded = result_of(
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
        let turn_id = loaded["_meta"]["srud"]["turns"][0]["turnId"]
            .as_str()
            .expect("the loaded session lists its turn")
            .to_string();

        let reply = tokio::time::timeout(
            Duration::from_secs(10),
            second.handle(request(
                SRUD_SESSION_REGENERATE,
                json!({ "sessionId": session_id.0.as_ref(), "turnId": turn_id }),
            )),
        )
        .await
        .expect("a regenerate resolves rather than being refused");
        let value = result_of(reply);
        assert_eq!(value["stopReason"], json!("end_turn"), "{value:?}");
    }
}
