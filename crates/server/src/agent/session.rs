//! Creating, naming and closing sessions.
//!
//! `session/new` registers a session against the directory the client named,
//! or against a workspace of its own when it named none; `session/list`
//! reports what the registry holds; `session/close` and `session/delete`
//! deregister it. A session's title comes from the message that opened it and
//! can be overridden by a client.

use super::{parse_params, session_not_found, Agent};
use serde_json::Value;
use srud_protocol::acp::{
    AcpError, CloseSessionRequest, DeleteSessionRequest, ListSessionsResponse, NewSessionRequest,
    NewSessionResponse, SessionId,
};
use srud_protocol::error::{INTERNAL_ERROR, INVALID_REQUEST};
use srud_protocol::srud::methods::{
    ForkSessionRequest, ForkSessionResponse, SetSessionTitleRequest, SetSessionTitleResponse,
};

impl Agent {
    pub(super) async fn handle_new_session(
        &self,
        params: Value,
    ) -> Result<NewSessionResponse, AcpError> {
        let request: NewSessionRequest = parse_params(params)?;
        // Seeded from the configured default so the session and its log agree
        // from the first record, and nothing later has to guess what it was
        // running on before anyone chose. Nothing is seeded when no default is
        // set — the session opens with the picker waiting rather than with a
        // model its user never named.
        let start = self
            .providers
            .as_ref()
            .and_then(|providers| providers.store.start_reference());
        let start_thinking = start.as_deref().and_then(|reference| {
            let providers = self.providers.as_ref()?;
            let (_, model) = providers.store.resolve(reference).ok()?;
            model
                .default_thinking()
                .map(|level| level.as_str().to_owned())
        });
        let session_id = self
            .sessions
            .create_with_thinking(Some(request.cwd), start, start_thinking)
            .await
            .map_err(|err| {
                AcpError::new(
                    INTERNAL_ERROR,
                    format!("could not create the session workspace: {err}"),
                )
            })?;
        let mut response = NewSessionResponse::new(session_id.clone());
        // Offered to a client that may render it, and ignored by one that does
        // not know the category — which is the whole reason it travels here
        // rather than through a method of SrudAgent's own.
        if let Some(options) = self.config_options(&session_id) {
            response = response.config_options(options);
        }
        Ok(response)
    }

    pub(super) async fn handle_list(&self) -> Result<ListSessionsResponse, AcpError> {
        Ok(ListSessionsResponse::new(self.sessions.list().await))
    }

    pub(super) async fn handle_close(&self, params: Value) -> Result<(), AcpError> {
        let request: CloseSessionRequest = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        session.interrupt();
        // Closing leaves the log where it is: the session is over, not deleted, and
        // is what the next `session/list` reports so a client can load it again.
        self.sessions.forget(&request.session_id);
        Ok(())
    }

    pub(super) async fn handle_delete(&self, params: Value) -> Result<(), AcpError> {
        let request: DeleteSessionRequest = parse_params(params)?;
        if !self.sessions.remove(&request.session_id).await {
            return Err(session_not_found(&request.session_id));
        }
        Ok(())
    }

    /// Names an unnamed session after the message that opened it.
    ///
    /// A session is named by its first prompt and never re-derived: later turns
    /// must not overwrite it, and a rename the user has already made wins. When
    /// this call is the one that named the session, it broadcasts the title so
    /// every attached client renders it without asking.
    ///
    /// A title that cannot be recorded is not announced, but does not fail the
    /// prompt: the name is cosmetic, and the turn it opened still has to run. The
    /// failure is dropped, which is the one place that is the right answer — a
    /// disk that refused the title will refuse the turn's own records a moment
    /// later and report it there.
    pub(super) async fn name_from_first_prompt(&self, session_id: &SessionId, text: &str) {
        let title = title_from_prompt(text);
        if let Ok(true) = self.sessions.name_if_unnamed(session_id, &title).await {
            self.broadcast_title(session_id, Some(title));
        }
    }

    pub(super) async fn handle_set_title(
        &self,
        params: Value,
    ) -> Result<SetSessionTitleResponse, AcpError> {
        let request: SetSessionTitleRequest = parse_params(params)?;
        match self
            .sessions
            .set_title(&request.session_id, &request.title)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(session_not_found(&request.session_id)),
            Err(err) => {
                return Err(AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot record the title: {err}"),
                ));
            }
        }
        // Echo the effective title rather than the request's, so a blank title
        // is reported as the clear it was.
        let title = self.sessions.title(&request.session_id);
        self.broadcast_title(&request.session_id, title.clone());
        Ok(SetSessionTitleResponse { title, meta: None })
    }

    /// Copies a session's history into a session of its own.
    ///
    /// The fork stands where the source stands and the source is left as it
    /// was. A turn in flight refuses the fork rather than copying one.
    pub(super) async fn handle_fork(&self, params: Value) -> Result<ForkSessionResponse, AcpError> {
        let request: ForkSessionRequest = parse_params(params)?;
        let session_id = self
            .sessions
            .fork(&request.session_id, request.title)
            .await
            .map_err(|err| match err {
                crate::sessions::ForkError::Restore(crate::sessions::RestoreError::NotFound(
                    id,
                )) => session_not_found(&id),
                crate::sessions::ForkError::Busy => AcpError::new(
                    INVALID_REQUEST,
                    "cannot fork a session while a turn is in flight",
                ),
                err => AcpError::new(INTERNAL_ERROR, format!("could not fork the session: {err}")),
            })?;
        Ok(ForkSessionResponse {
            session_id,
            meta: None,
        })
    }
}

/// Derives a session title from the message that opened it.
///
/// The message is a whole paragraph, so it is flattened to one line and cut to
/// a length a sidebar row can show. Truncation is on a character boundary, not
/// a byte one, so multi-byte text cannot panic.
fn title_from_prompt(text: &str) -> String {
    const MAX_CHARS: usize = 60;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_CHARS).collect();
    // Drop a trailing partial word so the title does not end mid-word.
    match cut.rfind(' ') {
        Some(idx) if idx > MAX_CHARS / 2 => cut[..idx].to_string(),
        _ => cut,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::agent::support::*;
    use serde_json::json;
    use srud_core::client::ModelEvent;
    use srud_protocol::acp::methods::{
        SESSION_CANCEL, SESSION_CLOSE, SESSION_DELETE, SESSION_LIST, SESSION_LOAD, SESSION_NEW,
        SESSION_PROMPT, SRUD_SESSION_FORK, SRUD_SESSION_SET_TITLE,
    };
    use srud_protocol::acp::{Response, SessionId};
    use srud_protocol::error::{INVALID_REQUEST, SESSION_NOT_FOUND};

    #[tokio::test]
    async fn session_new_returns_an_opaque_id() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let id = new_session(&agent).await;
        assert!(!id.0.is_empty());
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        // Counted rather than asserted as the only one: the configuration
        // directory comes from a process-wide environment variable that tests
        // beside this one also set, so the listing may be reading their home.
        assert!(
            listed["sessions"]
                .as_array()
                .is_some_and(|all| !all.is_empty()),
            "the session it just made is listed"
        );
        // The session works in the workspace created for it, named by its id —
        // not in the directory the client sent.
        let mine = listed["sessions"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|info| info["sessionId"] == json!(id.0.as_ref()))
            .expect("the session it just made is listed");
        let cwd = mine["cwd"]
            .as_str()
            .expect("the session reports a working directory");
        assert!(
            cwd.ends_with(id.0.as_ref()),
            "the workspace is named by the session id: {cwd}"
        );
        assert!(
            std::path::Path::new(cwd).is_dir(),
            "the workspace exists: {cwd}"
        );
    }

    #[tokio::test]
    async fn session_new_works_where_the_client_said_to() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let chosen = std::env::temp_dir().join(format!("srud-chosen-{}", std::process::id()));
        std::fs::create_dir_all(&chosen).expect("a directory to work in");

        let reply = agent
            .handle(request(
                SESSION_NEW,
                json!({ "cwd": chosen.to_str(), "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new should succeed");
        };
        let id = SessionId::new(result["sessionId"].as_str().unwrap().to_string());

        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        assert_eq!(listed["sessions"][0]["cwd"], chosen.to_str().unwrap());

        // A directory the client named is the user's, so deleting the session
        // must leave it standing.
        result_of(
            agent
                .handle(request(
                    SESSION_DELETE,
                    json!({ "sessionId": id.0.as_ref() }),
                ))
                .await,
        );
        assert!(chosen.is_dir(), "{chosen:?} is not the agent's to delete");
    }

    #[tokio::test]
    async fn the_first_prompt_names_the_session_and_broadcasts_it() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let titles = prompt_titles(&agent, &session_id, "Why is the timestamp wrong?").await;
        assert_eq!(
            titles,
            vec![Some("Why is the timestamp wrong?".to_string())],
            "the first prompt names the session once"
        );

        // A later turn must not rename it.
        let later = prompt_titles(&agent, &session_id, "A completely different question").await;
        assert!(
            later.is_empty(),
            "a later prompt does not rename the session"
        );
    }

    #[tokio::test]
    async fn set_title_renames_and_broadcasts() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let mut rx = agent.subscribe();
        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": session_id.0.as_ref(), "title": "Chosen by hand" }),
                ))
                .await,
        );
        assert_eq!(result["title"], json!("Chosen by hand"));

        let notification = rx.try_recv().expect("the rename is broadcast");
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["method"], json!("session/update"));
        assert_eq!(
            value["params"]["update"]["sessionUpdate"],
            json!("session_info_update")
        );
        assert_eq!(value["params"]["update"]["title"], json!("Chosen by hand"));

        // A rename outranks the derived name, so the next prompt leaves it alone.
        let titles = prompt_titles(&agent, &session_id, "another question").await;
        assert!(titles.is_empty());
    }

    #[tokio::test]
    async fn a_blank_rename_clears_the_title_and_reports_it() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        agent
            .handle(request(
                SRUD_SESSION_SET_TITLE,
                json!({ "sessionId": session_id.0.as_ref(), "title": "Named" }),
            ))
            .await;

        let mut rx = agent.subscribe();
        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": session_id.0.as_ref(), "title": "  " }),
                ))
                .await,
        );
        assert!(
            result.get("title").is_none(),
            "a blank title is reported as the clear it was"
        );

        let notification = rx.try_recv().expect("the clear is broadcast");
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["params"]["update"]["title"], json!(null));
    }

    #[tokio::test]
    async fn renaming_an_unknown_session_is_session_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": "missing", "title": "x" }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn list_reports_the_derived_title() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        prompt_titles(&agent, &session_id, "Name me").await;

        let result = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        let listed = result["sessions"]
            .as_array()
            .expect("sessions is an array")
            .iter()
            .find(|info| info["sessionId"] == json!(session_id.0.as_ref()))
            .expect("the prompted session is listed");
        assert_eq!(listed["title"], json!("Name me"));
    }

    #[tokio::test]
    async fn close_and_delete_deregister_the_session() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let reply = agent
            .handle(request(
                SESSION_CLOSE,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        assert!(matches!(reply.inner(), Response::Result { .. }));

        let error = error_of(
            agent
                .handle(request(
                    SESSION_DELETE,
                    json!({ "sessionId": session_id.0.as_ref() }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn fork_copies_the_conversation_and_names_the_fork() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-fork"),
        );
        let agent = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "answer".into(),
        }]));
        initialize(&agent).await;
        let source = new_session_in(&agent, "").await;
        agent
            .handle(request(
                SRUD_SESSION_SET_TITLE,
                json!({ "sessionId": source.0.as_ref(), "title": "Chosen" }),
            ))
            .await;
        prompt_titles(&agent, &source, "question").await;

        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_FORK,
                    json!({ "sessionId": source.0.as_ref() }),
                ))
                .await,
        );
        let forked = result["sessionId"]
            .as_str()
            .expect("the fork reports its id")
            .to_owned();
        assert_ne!(
            forked,
            source.0.as_ref(),
            "the fork is a session of its own"
        );

        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        let title = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["sessionId"] == json!(forked))
            .map(|entry| entry["title"].as_str().unwrap_or_default().to_owned())
            .expect("the fork is listed");
        assert_eq!(title, "Fork of Chosen");

        // The fork stands where the source stood: its conversation replays.
        let mut rx = agent.subscribe();
        agent
            .handle(request(
                SESSION_LOAD,
                json!({ "sessionId": forked, "cwd": "", "mcpServers": [] }),
            ))
            .await;
        let replayed = conversation(&mut rx);
        assert!(
            replayed
                .iter()
                .any(|(kind, text)| kind == "user_message_chunk" && text == "question"),
            "the fork replays the source's conversation: {replayed:?}"
        );
    }

    #[tokio::test]
    async fn fork_honours_the_title_it_is_given() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-fork"),
        );
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let source = new_session_in(&agent, "").await;

        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_FORK,
                    json!({ "sessionId": source.0.as_ref(), "title": "Side branch" }),
                ))
                .await,
        );
        let forked = result["sessionId"].as_str().unwrap();
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        let entry = listed["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["sessionId"] == json!(forked))
            .expect("the fork is listed");
        assert_eq!(entry["title"], json!("Side branch"));
    }

    #[tokio::test]
    async fn forking_an_unknown_session_is_session_not_found() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-fork"),
        );
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SRUD_SESSION_FORK,
                    json!({ "sessionId": "missing" }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn fork_refuses_a_session_with_a_turn_in_flight() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-fork"),
        );
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let source = new_session_in(&agent, "").await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = source.clone();
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
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let error = error_of(
            agent
                .handle(request(
                    SRUD_SESSION_FORK,
                    json!({ "sessionId": source.0.as_ref() }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_REQUEST);

        agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": source.0.as_ref() }),
            ))
            .await;
        prompt.await.unwrap();
    }
}
