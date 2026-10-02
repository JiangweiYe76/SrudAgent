//! The in-memory session registry.
//!
//! Owns the mapping between the wire's opaque `sessionId` strings and the
//! core runtime's [`Session`] values, and creates each session's working
//! directory. Sessions live only as long as the process: there is no rollout
//! store, so a restart empties the registry.
//!
//! Every session works in a directory of its own under the system temp
//! directory, so one session's files are never another's.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use srud_core::session::Session;
use srud_protocol::acp::{SessionId, SessionInfo};

/// The directory per-session workspaces are created under, inside the system
/// temp directory.
const WORKSPACES_DIR: &str = "SrudAgentWorkspaces";

/// A tracked session: the runtime handle plus the wire-level context.
struct Tracked {
    session: Arc<Session>,
    /// The agent's display title. `None` until the first prompt names the
    /// session, or until a client renames it.
    title: Option<String>,
}

fn workspace_for(id: srud_core::types::SessionId) -> PathBuf {
    std::env::temp_dir()
        .join(WORKSPACES_DIR)
        .join(id.to_string())
}

/// Collapses a candidate title to a single line, or drops it when it holds no
/// visible characters.
///
/// Titles are rendered in a single-line sidebar row, so embedded newlines and
/// runs of whitespace would be truncated into something meaningless.
fn normalise_title(title: &str) -> Option<String> {
    let collapsed = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

/// A registry of live sessions, keyed by their wire id.
#[derive(Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Tracked>>,
}

impl SessionManager {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a fresh session and returns its wire id.
    ///
    /// # Errors
    ///
    /// Returns the underlying I/O error if the session's working directory
    /// could not be created: a session without one cannot run tools.
    pub fn create(&self) -> std::io::Result<SessionId> {
        let session_id = srud_core::types::SessionId::new();
        let cwd = workspace_for(session_id);
        std::fs::create_dir_all(&cwd)?;
        let session = Arc::new(Session::with_id(session_id, cwd));
        let id = SessionId::new(session.id().to_string());
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .insert(
                id.to_string(),
                Tracked {
                    session,
                    title: None,
                },
            );
        Ok(id)
    }

    /// Sets a session's display title, returning whether the session existed.
    ///
    /// An empty or whitespace-only title clears it, returning the session to its
    /// unnamed state.
    pub fn set_title(&self, id: &SessionId, title: &str) -> bool {
        let title = normalise_title(title);
        let mut sessions = self.sessions.lock().expect("session map lock poisoned");
        let Some(tracked) = sessions.get_mut(id.0.as_ref()) else {
            return false;
        };
        tracked.title = title;
        true
    }

    /// Names a session only while it is still unnamed, returning whether the
    /// session now has the given title.
    ///
    /// Returns `false` if the session is unknown, already named, or the title
    /// holds nothing visible. This is how the first prompt claims the title
    /// without a later prompt overwriting a name the user has since chosen.
    /// Deciding and writing under one lock keeps two concurrent prompts from
    /// both believing they won.
    pub fn name_if_unnamed(&self, id: &SessionId, title: &str) -> bool {
        let Some(title) = normalise_title(title) else {
            return false;
        };
        let mut sessions = self.sessions.lock().expect("session map lock poisoned");
        let Some(tracked) = sessions.get_mut(id.0.as_ref()) else {
            return false;
        };
        if tracked.title.is_some() {
            return false;
        }
        tracked.title = Some(title);
        true
    }

    /// Reads a session's display title, if it has one.
    #[must_use]
    pub fn title(&self, id: &SessionId) -> Option<String> {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .get(id.0.as_ref())
            .and_then(|tracked| tracked.title.clone())
    }

    /// Looks up a session's runtime handle by wire id.
    #[must_use]
    pub fn get(&self, id: &SessionId) -> Option<Arc<Session>> {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .get(id.0.as_ref())
            .map(|tracked| Arc::clone(&tracked.session))
    }

    /// Removes a session, returning whether it existed.
    pub fn remove(&self, id: &SessionId) -> bool {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .remove(id.0.as_ref())
            .is_some()
    }

    /// Snapshots every live session as wire-level [`SessionInfo`] values.
    #[must_use]
    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .iter()
            .map(|(id, tracked)| {
                let mut info = SessionInfo::new(
                    SessionId::new(id.clone()),
                    tracked.session.cwd().to_path_buf(),
                );
                info.title = tracked.title.clone();
                info.updated_at = None;
                info
            })
            .collect()
    }

    /// Returns how many sessions are live.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .len()
    }

    /// Returns whether no session is live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Creates a session, failing the test rather than the caller when the
    /// workspace cannot be made.
    fn create(manager: &SessionManager) -> SessionId {
        manager.create().expect("the workspace is creatable")
    }

    #[test]
    fn create_then_get_round_trips_the_id() {
        let manager = SessionManager::new();
        let id = create(&manager);
        let session = manager.get(&id).expect("the session is registered");
        assert_eq!(session.id().to_string(), id.0.as_ref());
    }

    #[test]
    fn create_makes_a_working_directory_for_the_session() {
        let manager = SessionManager::new();
        let id = create(&manager);
        let session = manager.get(&id).expect("the session is registered");
        let cwd = session.cwd();
        assert!(cwd.is_dir(), "the working directory exists: {cwd:?}");

        let root = std::env::temp_dir().join(WORKSPACES_DIR);
        assert!(cwd.starts_with(&root), "{cwd:?} lives under {root:?}");
        assert_eq!(
            cwd.file_name().map(|name| name.to_string_lossy()),
            Some(id.0.as_ref().into()),
            "the directory is named by the session id"
        );
    }

    #[test]
    fn sessions_get_their_own_working_directory() {
        let manager = SessionManager::new();
        let a = manager.get(&create(&manager)).expect("registered");
        let b = manager.get(&create(&manager)).expect("registered");
        assert_ne!(a.cwd(), b.cwd());
    }

    #[test]
    fn unknown_id_resolves_to_none() {
        let manager = SessionManager::new();
        assert!(manager.get(&SessionId::new("nope")).is_none());
    }

    #[test]
    fn remove_drops_the_session() {
        let manager = SessionManager::new();
        let id = create(&manager);
        assert!(manager.remove(&id));
        assert!(!manager.remove(&id), "a second removal finds nothing");
        assert!(manager.is_empty());
    }

    #[test]
    fn a_new_session_has_no_title() {
        let manager = SessionManager::new();
        let id = create(&manager);
        assert_eq!(manager.title(&id), None);
        assert!(manager.list()[0].title.is_none());
    }

    #[test]
    fn name_if_unnamed_only_the_first_caller_wins() {
        let manager = SessionManager::new();
        let id = create(&manager);
        assert!(manager.name_if_unnamed(&id, "first message"));
        // A later prompt must not clobber the name the session already has.
        assert!(!manager.name_if_unnamed(&id, "second message"));
        assert_eq!(manager.title(&id).as_deref(), Some("first message"));
    }

    #[test]
    fn set_title_overwrites_and_reports_unknown_ids() {
        let manager = SessionManager::new();
        let id = create(&manager);
        assert!(manager.name_if_unnamed(&id, "derived"));
        assert!(manager.set_title(&id, "chosen by the user"));
        assert_eq!(manager.title(&id).as_deref(), Some("chosen by the user"));
        assert!(!manager.set_title(&SessionId::new("nope"), "x"));
    }

    #[test]
    fn a_blank_title_clears_the_name() {
        let manager = SessionManager::new();
        let id = create(&manager);
        manager.name_if_unnamed(&id, "named");
        assert!(manager.set_title(&id, "   \n  "));
        assert_eq!(manager.title(&id), None);
        // Cleared means unnamed, so the next prompt may name it again.
        assert!(manager.name_if_unnamed(&id, "named again"));
    }

    #[test]
    fn titles_collapse_to_a_single_line() {
        let manager = SessionManager::new();
        let id = create(&manager);
        manager.set_title(&id, "  fix   the\n\n  timestamp  ");
        assert_eq!(manager.title(&id).as_deref(), Some("fix the timestamp"));
    }

    #[test]
    fn list_reports_the_title() {
        let manager = SessionManager::new();
        let named = create(&manager);
        manager.set_title(&named, "Named session");
        let listed = manager.list();
        let titled = listed
            .iter()
            .find(|info| info.session_id == named)
            .expect("the named session is listed");
        assert_eq!(titled.title.as_deref(), Some("Named session"));
    }

    #[test]
    fn list_reports_the_working_directory_for_every_session() {
        let manager = SessionManager::new();
        let mut expected = [create(&manager), create(&manager)];
        expected.sort_by_key(ToString::to_string);

        let mut listed = manager.list();
        listed.sort_by_key(|info| info.session_id.to_string());

        let ids: Vec<String> = listed
            .iter()
            .map(|info| info.session_id.to_string())
            .collect();
        assert_eq!(
            ids,
            expected.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
        assert!(
            listed.iter().all(|info| info.cwd.is_dir()),
            "every listed session reports the directory it works in"
        );
    }
}
