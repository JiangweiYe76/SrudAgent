//! The in-memory session registry.
//!
//! Owns the mapping between the wire's opaque `sessionId` strings and the
//! core runtime's [`Session`] values, plus the per-session `cwd` the runtime
//! itself does not track. Sessions live only as long as the process: there is
//! no rollout store, so a restart empties the registry.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use srud_core::session::Session;
use srud_protocol::acp::{SessionId, SessionInfo};

/// A tracked session: the runtime handle plus the wire-level context.
struct Tracked {
    session: Arc<Session>,
    cwd: PathBuf,
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

    /// Registers a fresh session for `cwd` and returns its wire id.
    pub fn create(&self, cwd: PathBuf) -> SessionId {
        let session = Arc::new(Session::new());
        let id = SessionId::new(session.id().to_string());
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .insert(id.to_string(), Tracked { session, cwd });
        id
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
                let mut info = SessionInfo::new(SessionId::new(id.clone()), tracked.cwd.clone());
                info.title = None;
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

    #[test]
    fn create_then_get_round_trips_the_id() {
        let manager = SessionManager::new();
        let id = manager.create(PathBuf::from("/tmp/a"));
        let session = manager.get(&id).expect("the session is registered");
        assert_eq!(session.id().to_string(), id.0.as_ref());
    }

    #[test]
    fn unknown_id_resolves_to_none() {
        let manager = SessionManager::new();
        assert!(manager.get(&SessionId::new("nope")).is_none());
    }

    #[test]
    fn remove_drops_the_session() {
        let manager = SessionManager::new();
        let id = manager.create(PathBuf::from("/tmp/a"));
        assert!(manager.remove(&id));
        assert!(!manager.remove(&id), "a second removal finds nothing");
        assert!(manager.is_empty());
    }

    #[test]
    fn list_reports_cwd_for_every_session() {
        let manager = SessionManager::new();
        let a = manager.create(PathBuf::from("/tmp/a"));
        let b = manager.create(PathBuf::from("/tmp/b"));
        let mut listed = manager.list();
        listed.sort_by_key(|x| x.session_id.to_string());
        let mut expected = vec![a.to_string(), b.to_string()];
        expected.sort();
        let ids: Vec<String> = listed
            .iter()
            .map(|info| info.session_id.to_string())
            .collect();
        assert_eq!(ids, expected);
        assert!(listed.iter().all(|info| !info.cwd.as_os_str().is_empty()));
    }
}
