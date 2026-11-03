//! The sandbox mode a session runs in.
//!
//! Full access is the mode this build advertises. It travels as an ACP session
//! mode rather than a config option: a mode describes what the session is, and a
//! client that knows nothing about SrudAgent can still render and switch it.

use super::{parse_params, session_not_found, Agent};
use serde_json::Value;
use srud_core::sandbox::SandboxLevel;
use srud_protocol::acp::{
    AcpError, CurrentModeUpdate, Notification, SessionId, SessionMode, SessionModeState,
    SessionNotification, SessionUpdate, SetSessionModeRequest, SetSessionModeResponse,
    CLIENT_METHOD_NAMES,
};
use srud_protocol::error::{INTERNAL_ERROR, INVALID_PARAMS};

impl Agent {
    /// The mode state a session is offered.
    ///
    /// `None` for a session that does not exist. The available list is the set
    /// this build advertises: workspace write and full access.
    pub(super) fn mode_state(&self, id: &SessionId) -> Option<SessionModeState> {
        let level = self.sessions.sandbox_level(id)?;
        Some(SessionModeState::new(
            level.id(),
            available_modes().to_vec(),
        ))
    }

    /// Switches a session's mode.
    ///
    /// Only a mode this build advertises is accepted. A level outside the
    /// advertised set is refused rather than silently run with a different
    /// boundary.
    pub(super) async fn handle_set_mode(
        &self,
        params: Value,
    ) -> Result<SetSessionModeResponse, AcpError> {
        let request: SetSessionModeRequest = parse_params(params)?;
        let Some(level) = SandboxLevel::from_id(request.mode_id.0.as_ref()) else {
            return Err(AcpError::new(
                INVALID_PARAMS,
                format!("unknown session mode `{}`", request.mode_id.0),
            ));
        };
        if !available_modes()
            .iter()
            .any(|mode| mode.id == request.mode_id)
        {
            return Err(AcpError::new(
                INVALID_PARAMS,
                format!("session mode `{}` is not available", request.mode_id.0),
            ));
        }

        match self
            .sessions
            .set_sandbox_level(&request.session_id, level)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(session_not_found(&request.session_id)),
            Err(err) => {
                return Err(AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot record the session mode: {err}"),
                ));
            }
        }

        self.hub.send(Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                request.session_id.clone(),
                SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(level.id())),
            )),
        });
        Ok(SetSessionModeResponse::new())
    }
}

/// The modes this build can run.
fn available_modes() -> &'static [SessionMode] {
    // Leaked once rather than rebuilt per request: the set is fixed, and a
    // client compares ids across responses.
    static MODES: std::sync::OnceLock<Vec<SessionMode>> = std::sync::OnceLock::new();
    MODES.get_or_init(|| {
        vec![
            SessionMode::new(
                SandboxLevel::WorkspaceWrite.id(),
                SandboxLevel::WorkspaceWrite.name(),
            )
            .description(SandboxLevel::WorkspaceWrite.description()),
            SessionMode::new(
                SandboxLevel::FullAccess.id(),
                SandboxLevel::FullAccess.name(),
            )
            .description(SandboxLevel::FullAccess.description()),
        ]
    })
}

#[cfg(test)]
mod tests {
    use crate::agent::support::*;
    use srud_protocol::acp::methods::SESSION_SET_MODE;
    use srud_protocol::error::INVALID_PARAMS;

    #[tokio::test]
    async fn a_new_session_offers_workspace_write() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let result = new_session_result(&agent).await;

        assert_eq!(result["modes"]["currentModeId"], "workspace-write");
        assert_eq!(
            result["modes"]["availableModes"][0]["id"],
            "workspace-write"
        );
        assert_eq!(result["modes"]["availableModes"][1]["id"], "full-access");
    }

    #[tokio::test]
    async fn full_access_is_accepted() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session_result(&agent).await["sessionId"].clone();

        result_of(
            agent
                .handle(request(
                    SESSION_SET_MODE,
                    serde_json::json!({ "sessionId": session_id, "modeId": "full-access" }),
                ))
                .await,
        );
    }

    #[tokio::test]
    async fn a_mode_outside_the_advertised_set_is_refused() {
        let _home = home();
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session_result(&agent).await["sessionId"].clone();

        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_MODE,
                    serde_json::json!({ "sessionId": session_id, "modeId": "read-only" }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }
}
