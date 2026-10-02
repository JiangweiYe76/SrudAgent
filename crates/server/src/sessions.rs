//! The in-memory session registry.
//!
//! Owns the mapping between the wire's opaque `sessionId` strings and the
//! core runtime's [`Session`] values, and settles each session's working
//! directory. Sessions live only as long as the process: there is no rollout
//! store, so a restart empties the registry.
//!
//! A session works in a directory the client named, or in one of its own under
//! the configuration directory, so one session's files are never another's.
//! Removing a session removes a workspace this agent created for it; a directory
//! the user pointed at is theirs and is left alone.
//!
//! The second half of that is a departure from ACP, which requires a session to
//! work in the directory [`session/new`] named. [`usable_directory`] says when
//! the agent overrides that and why.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use srud_core::session::Session;
use srud_protocol::acp::{SessionId, SessionInfo};

use crate::config;

/// The directory per-session workspaces are created under, inside the
/// configuration directory.
const WORKSPACES_DIR: &str = "workspaces";

/// A tracked session: the runtime handle plus the wire-level context.
struct Tracked {
    session: Arc<Session>,
    /// The agent's display title. `None` until the first prompt names the
    /// session, or until a client renames it.
    title: Option<String>,
    /// Whether the working directory is one this agent created, and so may
    /// remove. A directory the client named belongs to the user.
    workspace_is_ours: bool,
}

/// The workspace for a session that named no directory of its own.
fn workspace_for(id: srud_core::types::SessionId) -> Result<PathBuf, config::ConfigError> {
    config::home()
        .map(|home| home.join(WORKSPACES_DIR).join(id.to_string()))
        .ok_or(config::ConfigError::NoHome)
}

/// The directory a client named, if it named one the session can work in.
///
/// ACP makes the working directory part of `session/new`, requires it to be an
/// absolute path, and requires the agent to use it for the session regardless
/// of where the agent itself was spawned:
/// <https://agentclientprotocol.com/protocol/session-setup#working-directory>
///
/// SrudAgent also keeps a session that works on its own, and this is where the
/// client's choice yields to it. A client that named nothing, a path that has
/// since been deleted, and a file where a directory belongs all mean the same
/// thing — the session has no working directory to work in — and each of them
/// gives the session a workspace of its own instead. That is a deliberate
/// departure from the rule above.
///
/// A relative path is refused rather than resolved. Resolving it against the
/// agent process's own directory would give an answer that holds only until the
/// agent is spawned somewhere else, which is the situation the same rule exists
/// to rule out.
fn usable_directory(cwd: Option<PathBuf>) -> Option<PathBuf> {
    cwd.filter(|dir| dir.is_absolute() && dir.is_dir())
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

/// Why a session could not be started.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// There is no configuration directory to put the session's workspace in.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// The workspace could not be created.
    #[error("cannot create {path}: {source}")]
    Workspace {
        /// The directory that could not be created.
        path: PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },
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
    /// `cwd` is where the client wants the session to work. A directory that
    /// exists is used as it stands and nothing is created for it. Anything else
    /// — nothing at all, a path that is not there, a file, a relative path —
    /// leaves the session with a workspace of its own under the configuration
    /// directory; see [`usable_directory`] for why.
    ///
    /// # Errors
    ///
    /// Returns [`CreateError::Config`] when the session needs a workspace and
    /// there is no configuration directory to put it in, and
    /// [`CreateError::Workspace`] when that workspace cannot be created. A
    /// session with no working directory cannot call a tool, so neither failure
    /// leaves a session half-started.
    pub fn create(&self, cwd: Option<PathBuf>) -> Result<SessionId, CreateError> {
        let session_id = srud_core::types::SessionId::new();
        let (cwd, workspace_is_ours) = match usable_directory(cwd) {
            Some(cwd) => (cwd, false),
            None => {
                let path = workspace_for(session_id)?;
                std::fs::create_dir_all(&path).map_err(|source| CreateError::Workspace {
                    path: path.clone(),
                    source,
                })?;
                (path, true)
            }
        };
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
                    workspace_is_ours,
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
    ///
    /// A workspace this agent created goes with it. A directory the client named
    /// does not: the session record is the agent's, the files in there are the
    /// user's. Failing to delete leaves files behind rather than failing the
    /// call, because the session is gone either way and the caller has nothing
    /// useful to do about a directory that would not go.
    pub fn remove(&self, id: &SessionId) -> bool {
        let removed = self
            .sessions
            .lock()
            .expect("session map lock poisoned")
            .remove(id.0.as_ref());
        let Some(tracked) = removed else {
            return false;
        };
        if tracked.workspace_is_ours {
            let _ = std::fs::remove_dir_all(tracked.session.cwd());
        }
        true
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
    use std::env;

    use crate::config::HOME_VAR;
    use crate::test_env::{self, Guard};

    use super::*;

    /// A manager whose default workspaces land under a configuration directory
    /// of its own, so a test never writes into the developer's home.
    fn manager() -> (SessionManager, Guard) {
        let guard = Guard::take(&[HOME_VAR]);
        env::set_var(HOME_VAR, test_env::unique_dir("srud-sessions"));
        (SessionManager::new(), guard)
    }

    /// Creates a session with no directory of its own asked for, failing the
    /// test rather than the caller when the workspace cannot be made.
    fn create(manager: &SessionManager) -> SessionId {
        manager.create(None).expect("the workspace is creatable")
    }

    #[test]
    fn create_then_get_round_trips_the_id() {
        let (manager, _env) = manager();
        let id = create(&manager);
        let session = manager.get(&id).expect("the session is registered");
        assert_eq!(session.id().to_string(), id.0.as_ref());
    }

    #[test]
    fn create_makes_a_working_directory_for_the_session() {
        let (manager, _env) = manager();
        let id = create(&manager);
        let session = manager.get(&id).expect("the session is registered");
        let cwd = session.cwd();
        assert!(cwd.is_dir(), "the working directory exists: {cwd:?}");

        let root = config::home()
            .expect("a configuration directory")
            .join(WORKSPACES_DIR);
        assert!(cwd.starts_with(&root), "{cwd:?} lives under {root:?}");
        assert_eq!(
            cwd.file_name().map(|name| name.to_string_lossy()),
            Some(id.0.as_ref().into()),
            "the directory is named by the session id"
        );
    }

    #[test]
    fn sessions_get_their_own_working_directory() {
        let (manager, _env) = manager();
        let a = manager.get(&create(&manager)).expect("registered");
        let b = manager.get(&create(&manager)).expect("registered");
        assert_ne!(a.cwd(), b.cwd());
    }

    #[test]
    fn a_directory_the_client_named_is_used_as_it_stands() {
        let (manager, _env) = manager();
        let chosen = test_env::unique_dir("srud-chosen");

        let id = manager
            .create(Some(chosen.clone()))
            .expect("no workspace is needed");

        assert_eq!(manager.get(&id).expect("registered").cwd(), chosen);
        let workspaces = config::home()
            .expect("a configuration directory")
            .join(WORKSPACES_DIR);
        assert!(
            !workspaces.exists(),
            "a named directory is used, not one made for the session: {}",
            workspaces.display()
        );
    }

    /// The workspace the session would have been given, whether or not it has
    /// been created yet.
    fn workspaces_root() -> PathBuf {
        config::home()
            .expect("a configuration directory")
            .join(WORKSPACES_DIR)
    }

    /// Asserts a session was given a workspace of its own rather than working
    /// where it was pointed.
    fn assert_given_a_workspace(manager: &SessionManager, id: &SessionId, why: &str) {
        let cwd = manager.get(id).expect("registered").cwd().to_path_buf();
        assert!(cwd.is_dir(), "{why}, and it exists: {cwd:?}");
        assert!(
            cwd.starts_with(workspaces_root()),
            "{why}: {cwd:?} is not under {}",
            workspaces_root().display()
        );
    }

    #[test]
    fn nothing_named_for_the_client_means_a_workspace() {
        let (manager, _env) = manager();
        let id = manager.create(Some(PathBuf::new())).expect("created");

        assert_given_a_workspace(&manager, &id, "an empty path names no directory");
    }

    #[test]
    fn a_path_that_does_not_exist_means_a_workspace() {
        let (manager, _env) = manager();
        let gone = test_env::unique_dir("srud-gone").join("never-created");

        let id = manager.create(Some(gone)).expect("created");

        assert_given_a_workspace(&manager, &id, "a directory that is not there");
    }

    #[test]
    fn a_file_where_a_directory_belongs_means_a_workspace() {
        let (manager, _env) = manager();
        let file = test_env::unique_dir("srud-file").join("a-file");
        std::fs::write(&file, "not a directory").expect("a file");

        let id = manager.create(Some(file)).expect("created");

        assert_given_a_workspace(&manager, &id, "a file is not a working directory");
    }

    #[test]
    fn a_relative_path_means_a_workspace() {
        let (manager, _env) = manager();

        let id = manager
            .create(Some(PathBuf::from("relative/path")))
            .expect("created");

        assert_given_a_workspace(
            &manager,
            &id,
            "a relative path would resolve against the agent's own directory",
        );
    }

    #[test]
    fn removing_a_session_removes_the_workspace_it_created() {
        let (manager, _env) = manager();
        let id = create(&manager);
        let cwd = manager.get(&id).expect("registered").cwd().to_path_buf();

        assert!(manager.remove(&id));

        assert!(!cwd.exists(), "{cwd:?} went with the session");
    }

    #[test]
    fn removing_a_session_leaves_a_directory_the_client_named() {
        let (manager, _env) = manager();
        let chosen = test_env::unique_dir("srud-chosen");
        let id = manager.create(Some(chosen.clone())).expect("created");

        assert!(manager.remove(&id));

        assert!(
            chosen.exists(),
            "the user's files are not the agent's to delete"
        );
    }

    #[test]
    fn unknown_id_resolves_to_none() {
        let (manager, _env) = manager();
        assert!(manager.get(&SessionId::new("nope")).is_none());
    }

    #[test]
    fn remove_drops_the_session() {
        let (manager, _env) = manager();
        let id = create(&manager);
        assert!(manager.remove(&id));
        assert!(!manager.remove(&id), "a second removal finds nothing");
        assert!(manager.is_empty());
    }

    #[test]
    fn a_new_session_has_no_title() {
        let (manager, _env) = manager();
        let id = create(&manager);
        assert_eq!(manager.title(&id), None);
        assert!(manager.list()[0].title.is_none());
    }

    #[test]
    fn name_if_unnamed_only_the_first_caller_wins() {
        let (manager, _env) = manager();
        let id = create(&manager);
        assert!(manager.name_if_unnamed(&id, "first message"));
        // A later prompt must not clobber the name the session already has.
        assert!(!manager.name_if_unnamed(&id, "second message"));
        assert_eq!(manager.title(&id).as_deref(), Some("first message"));
    }

    #[test]
    fn set_title_overwrites_and_reports_unknown_ids() {
        let (manager, _env) = manager();
        let id = create(&manager);
        assert!(manager.name_if_unnamed(&id, "derived"));
        assert!(manager.set_title(&id, "chosen by the user"));
        assert_eq!(manager.title(&id).as_deref(), Some("chosen by the user"));
        assert!(!manager.set_title(&SessionId::new("nope"), "x"));
    }

    #[test]
    fn a_blank_title_clears_the_name() {
        let (manager, _env) = manager();
        let id = create(&manager);
        manager.name_if_unnamed(&id, "named");
        assert!(manager.set_title(&id, "   \n  "));
        assert_eq!(manager.title(&id), None);
        // Cleared means unnamed, so the next prompt may name it again.
        assert!(manager.name_if_unnamed(&id, "named again"));
    }

    #[test]
    fn titles_collapse_to_a_single_line() {
        let (manager, _env) = manager();
        let id = create(&manager);
        manager.set_title(&id, "  fix   the\n\n  timestamp  ");
        assert_eq!(manager.title(&id).as_deref(), Some("fix the timestamp"));
    }

    #[test]
    fn list_reports_the_title() {
        let (manager, _env) = manager();
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
        let (manager, _env) = manager();
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
