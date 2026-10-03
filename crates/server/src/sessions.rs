//! The session registry.
//!
//! Owns the mapping between the wire's opaque `sessionId` strings and the
//! core runtime's [`Session`] values, settles each session's working directory,
//! and holds the log that session's turns are recorded into.
//!
//! **The registry is in-memory, but the sessions are not**: a restart empties the
//! map, and `session/load` and `session/resume` rebuild an entry from the log the
//! session left behind. [`find_log`] locates one by the id in its filename, which
//! is what stands in for an index until there is one.
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
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Datelike;
use srud_core::session::Session;
use srud_core::session_event::SessionEvent;
use srud_core::session_log::{RecordError, SessionLog};
use srud_core::session_store::{Loaded, SessionLogWriter};
use srud_protocol::acp::{SessionId, SessionInfo};

use crate::config;

/// The directory per-session workspaces are created under, inside the
/// configuration directory.
const WORKSPACES_DIR: &str = "workspaces";

/// The directory session logs are written under, beside the workspaces.
///
/// Separate from the workspaces because one is a place to work and the other is a
/// record of what happened: a session's log outlives its workspace, and a
/// workspace is removed when the session is.
const SESSIONS_DIR: &str = "sessions";

/// A tracked session: the runtime handle plus the wire-level context.
struct Tracked {
    session: Arc<Session>,
    /// Where this session's turns are recorded.
    ///
    /// Held beside the session rather than looked up per turn so that a turn and
    /// its log cannot come from different sessions: the two are registered
    /// together and are never separated.
    log: Arc<srud_core::session_store::SessionLogWriter>,
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

/// Whether two directories are the one directory.
///
/// Canonicalised when both can be, so a client naming `/w/` or a symlink to it
/// means the same directory as the written path. Compared as written when they
/// cannot: a workspace can be gone by the time its session is loaded, and then
/// the only thing to compare is the text.
fn same_directory(recorded: &Path, requested: &Path) -> bool {
    match (recorded.canonicalize(), requested.canonicalize()) {
        (Ok(recorded), Ok(requested)) => recorded == requested,
        _ => recorded == requested,
    }
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

    /// The session's log could not be opened or written.
    ///
    /// A session that cannot record is a session that cannot be recovered, and one
    /// that is announced before this fails would refuse every turn afterwards —
    /// so this is reported at creation instead, where the client can start
    /// another.
    #[error("cannot record {what}: {source}")]
    Log {
        /// What was being written.
        what: String,
        /// What the filesystem said.
        source: std::io::Error,
    },
}

/// Why a session could not be brought back from its log.
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    /// There is no configuration directory to look in.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// No log holds that session.
    #[error("no log for session {0}")]
    NotFound(SessionId),

    /// The log could not be read, or names no session.
    #[error(transparent)]
    Load(#[from] srud_core::session_store::LoadError),

    /// The caller named a different working directory from the one recorded.
    ///
    /// ACP requires the two to match, and they have to: the tools resolve relative
    /// paths against it, and the recorded history was produced somewhere.
    #[error("the session works in {recorded}, not {requested}")]
    Cwd {
        /// The directory the log records.
        recorded: PathBuf,
        /// The directory the caller asked for.
        requested: PathBuf,
    },

    /// A repair the log needed could not be written back.
    #[error(transparent)]
    Repair(#[from] srud_core::session_log::RecordError),
}

/// Finds the log a session was written to.
///
/// Searched rather than computed: the filename carries the moment the session was
/// created, which only the write path knew. An index would answer this in one
/// lookup, and this is what stands in for one until then.
async fn find_log(id: &SessionId) -> Result<PathBuf, RestoreError> {
    let home = config::home().ok_or(config::ConfigError::NoHome)?;
    let suffix = format!("-{id}.jsonl");

    let mut dirs = vec![home.join(SESSIONS_DIR)];
    while let Some(dir) = dirs.pop() {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            // A date directory this build never wrote, or one being pruned.
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if entry.file_type().await.is_ok_and(|kind| kind.is_dir()) {
                dirs.push(path);
            } else if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(&suffix))
            {
                return Ok(path);
            }
        }
    }
    Err(RestoreError::NotFound(id.clone()))
}

/// Opens the log for a session about to work in `cwd`.
///
/// Laid out by date, the way the design has it: `sessions/YYYY/MM/DD/session-<ts>-<id>.jsonl`.
/// The date is the session's own creation date rather than the turn's, so every
/// turn of one session lands in one file and the path reads as when the
/// conversation began.
///
/// Local time, not UTC, for the same reason `context` uses it: a reader looking for
/// today's sessions means their own today. A user east of Greenwich would
/// otherwise find a session they started after midnight filed under the day
/// before.
async fn open_log(
    session_id: srud_core::types::SessionId,
) -> Result<srud_core::session_store::SessionLogWriter, CreateError> {
    let now = chrono::Local::now();
    let home = config::home().ok_or(config::ConfigError::NoHome)?;

    let path = home.join(SESSIONS_DIR).join(format!(
        "{:04}/{:02}/{:02}/session-{}-{session_id}.jsonl",
        now.year(),
        now.month(),
        now.day(),
        now.format("%Y%m%dT%H%M%S"),
    ));

    srud_core::session_store::SessionLogWriter::create(path)
        .await
        .map_err(|source| CreateError::Log {
            what: "the session's log".into(),
            source,
        })
}

/// A registry of live sessions, keyed by their wire id.
#[derive(Default)]
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Tracked>>,
    /// Serialises title changes.
    ///
    /// Async, and taken for the whole change, because deciding the title and
    /// recording it straddle an await: the sessions map is behind a std lock, and
    /// holding one across the write is not `Send`. Without this, two changes could
    /// record in one order and apply in another.
    titles: tokio::sync::Mutex<()>,
    /// Serialises restores.
    ///
    /// The repairs a log needs are derived from what it holds, so two restores
    /// running at once would both derive them and both write them — leaving a
    /// tool call answered twice, which a model API will not take.
    restoring: tokio::sync::Mutex<()>,
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
    pub async fn create(&self, cwd: Option<PathBuf>) -> Result<SessionId, CreateError> {
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

        // The log is opened before the session is registered, so a session that
        // cannot record is never announced as created. Registering first and
        // failing after would leave a session that answers `session/new` and then
        // refuses every turn.
        let log = open_log(session_id).await?;

        let session = Arc::new(Session::with_id(session_id, cwd.clone()));
        let id = SessionId::new(session.id().to_string());

        // The session's own record, written once here rather than per turn: a log
        // that reopened with a header on every turn would give a reader as many
        // candidates for "where this session started" as the session had turns.
        // The path above dates by the local day, which is the one a person looking
        // for today's sessions means. A line's time is UTC instead: it is ordered
        // as machine time, and converted where a person reads it.
        log.record(&srud_core::session_event::SessionEvent::session(
            session_id, cwd,
        ))
        .await
        .map_err(|source| CreateError::Log {
            what: "the session's first record".into(),
            source: std::io::Error::other(source),
        })?;

        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .insert(
                id.to_string(),
                Tracked {
                    session,
                    log: Arc::new(log),
                    title: None,
                    workspace_is_ours,
                },
            );
        Ok(id)
    }

    /// Brings a session back from its log, returning what was rebuilt.
    ///
    /// The repairs the log needed are written before the session is registered, for
    /// the same reason a new session's first record is: what answers has to be a
    /// session whose log and history agree. A restore of a session that is already
    /// live rebuilds from the log and records nothing — the repairs are already
    /// there, and a second writer on one log would interleave with the first.
    ///
    /// # Errors
    ///
    /// [`RestoreError::NotFound`] when no log holds that session,
    /// [`RestoreError::Cwd`] when the caller names a different directory from the
    /// one recorded, and [`RestoreError::Load`] or [`RestoreError::Repair`] when
    /// the log cannot be read or written.
    pub async fn restore(&self, id: &SessionId, cwd: &Path) -> Result<Loaded, RestoreError> {
        let _restoring = self.restoring.lock().await;

        let live = self
            .sessions
            .lock()
            .expect("session map lock poisoned")
            .contains_key(id.0.as_ref());
        let path = find_log(id).await?;

        // A session that is still open has not stopped, so nothing is repaired
        // and nothing is written: its gaps have not happened yet. It is already
        // registered, and a second writer on one log would interleave with the
        // first.
        let loaded = if live {
            srud_core::session_store::load_running(&path).await?
        } else {
            srud_core::session_store::load(&path).await?
        };
        if !same_directory(&loaded.cwd, cwd) {
            return Err(RestoreError::Cwd {
                recorded: loaded.cwd.clone(),
                requested: cwd.to_path_buf(),
            });
        }
        if live {
            return Ok(loaded);
        }

        let log = SessionLogWriter::create(&path).await.map_err(|source| {
            RestoreError::Repair(srud_core::session_log::RecordError::Io {
                what: "the session's log".into(),
                source,
            })
        })?;
        for repair in &loaded.repairs {
            SessionLog::record(&log, repair).await?;
        }

        let session = Arc::new(Session::restored(
            loaded.session_id,
            loaded.cwd.clone(),
            loaded.history.clone(),
        ));
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .insert(
                id.to_string(),
                Tracked {
                    session,
                    log: Arc::new(log),
                    title: loaded.title.clone(),
                    // A directory this agent made is one it may remove again; the
                    // recorded cwd says which it was.
                    workspace_is_ours: workspace_for(loaded.session_id)
                        .is_ok_and(|own| own == loaded.cwd),
                },
            );
        Ok(loaded)
    }

    /// A session and its log, if the session exists.
    ///
    /// Both or neither: a turn needs the two together, and handing them out
    /// separately would let a caller pair one session's history with another's log.
    /// The log is registered beside the session and never replaced, so one lookup
    /// can return both.
    pub fn with_log(
        &self,
        id: &SessionId,
    ) -> Option<(
        Arc<Session>,
        Arc<srud_core::session_store::SessionLogWriter>,
    )> {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .get(id.0.as_ref())
            .map(|tracked| (Arc::clone(&tracked.session), Arc::clone(&tracked.log)))
    }

    /// Sets a session's display title, returning whether the session existed.
    ///
    /// An empty or whitespace-only title clears it, returning the session to its
    /// unnamed state.
    ///
    /// # Errors
    ///
    /// If the title could not be recorded. The change is not applied in that case:
    /// a title a reader cannot find later is one the session does not have.
    pub async fn set_title(&self, id: &SessionId, title: &str) -> Result<bool, RecordError> {
        let _changing = self.titles.lock().await;
        let title = normalise_title(title);
        let Some(log) = self.log_of(id) else {
            return Ok(false);
        };
        SessionLog::record(log.as_ref(), &SessionEvent::title(title.clone())).await?;

        let mut sessions = self.sessions.lock().expect("session map lock poisoned");
        let Some(tracked) = sessions.get_mut(id.0.as_ref()) else {
            // Gone between the record and here, so nothing was renamed. The title
            // record is already in a log that is going with it.
            return Ok(false);
        };
        tracked.title = title;
        Ok(true)
    }

    /// Names a session only while it is still unnamed, returning whether the
    /// session now has the given title.
    ///
    /// Returns `false` if the session is unknown, already named, or the title
    /// holds nothing visible. This is how the first prompt claims the title
    /// without a later prompt overwriting a name the user has since chosen.
    ///
    /// Reading the current title and writing the record are one step under the
    /// titles lock, so two prompts cannot both believe they won.
    ///
    /// # Errors
    ///
    /// As [`SessionManager::set_title`].
    pub async fn name_if_unnamed(&self, id: &SessionId, title: &str) -> Result<bool, RecordError> {
        let _changing = self.titles.lock().await;
        let Some(title) = normalise_title(title) else {
            return Ok(false);
        };
        let Some(log) = self.log_of(id) else {
            return Ok(false);
        };
        if self.title(id).is_some() {
            return Ok(false);
        }
        SessionLog::record(log.as_ref(), &SessionEvent::title(Some(title.clone()))).await?;

        let mut sessions = self.sessions.lock().expect("session map lock poisoned");
        let Some(tracked) = sessions.get_mut(id.0.as_ref()) else {
            return Ok(false);
        };
        tracked.title = Some(title);
        Ok(true)
    }

    /// The log a session writes to, if the session exists.
    fn log_of(&self, id: &SessionId) -> Option<Arc<SessionLogWriter>> {
        self.sessions
            .lock()
            .expect("session map lock poisoned")
            .get(id.0.as_ref())
            .map(|tracked| Arc::clone(&tracked.log))
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
    async fn create(manager: &SessionManager) -> SessionId {
        manager
            .create(None)
            .await
            .expect("the workspace is creatable")
    }

    /// The path of a session's log, found by the id in its filename.
    ///
    /// Searched rather than recomputed: the name carries the moment the session
    /// was created, which only the write path knows.
    fn session_log_path(id: &SessionId) -> PathBuf {
        let home = env::var(HOME_VAR).expect("the test home");
        let mut dirs = vec![PathBuf::from(home).join(SESSIONS_DIR)];
        while let Some(dir) = dirs.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .ends_with(&format!("-{id}.jsonl"))
                {
                    return path;
                }
            }
        }
        panic!("no log for session {id}");
    }

    /// The title records a session's log holds, oldest first.
    async fn recorded_titles(id: &SessionId) -> Vec<Option<String>> {
        let path = session_log_path(id);
        srud_core::session_store::read(&path)
            .await
            .expect("a log to read")
            .entries
            .into_iter()
            .filter_map(|entry| match entry.event {
                SessionEvent::Title { title } => Some(title),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn every_title_change_reaches_the_log() {
        // The title is not derivable — a model wrote it, or a client chose it — so
        // a session that cannot find it in its log comes back unnamed. Clearing
        // has to be recorded too, or the cleared name comes back.
        let (manager, _env) = manager();
        let id = create(&manager).await;

        manager.name_if_unnamed(&id, "derived").await.unwrap();
        manager.set_title(&id, "chosen by the user").await.unwrap();
        manager.set_title(&id, "   ").await.unwrap();

        assert_eq!(
            recorded_titles(&id).await,
            vec![
                Some("derived".to_string()),
                Some("chosen by the user".to_string()),
                // `None` is a cleared title, written rather than omitted.
                None,
            ]
        );
    }

    #[tokio::test]
    async fn a_second_prompt_does_not_record_the_name_it_lost() {
        // Latest-wins, so an extra record would be the title a reader took even
        // though the session never adopted it.
        let (manager, _env) = manager();
        let id = create(&manager).await;

        manager.name_if_unnamed(&id, "first").await.unwrap();
        assert!(!manager.name_if_unnamed(&id, "second").await.unwrap());

        assert_eq!(recorded_titles(&id).await, vec![Some("first".to_string())]);
    }

    #[test]
    fn a_directory_named_another_way_is_the_same_directory() {
        // ACP has the client name the session's cwd again on load, and a trailing
        // separator is not a different directory.
        let dir = std::env::temp_dir();
        let with_slash = dir.join("");

        assert!(same_directory(&dir, &with_slash));
        assert!(!same_directory(&dir, Path::new("/definitely/not/this")));
    }

    #[tokio::test]
    async fn create_then_get_round_trips_the_id() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        let session = manager.get(&id).expect("the session is registered");
        assert_eq!(session.id().to_string(), id.0.as_ref());
    }

    #[tokio::test]
    async fn create_makes_a_working_directory_for_the_session() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
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

    #[tokio::test]
    async fn sessions_get_their_own_working_directory() {
        let (manager, _env) = manager();
        let a = manager.get(&create(&manager).await).expect("registered");
        let b = manager.get(&create(&manager).await).expect("registered");
        assert_ne!(a.cwd(), b.cwd());
    }

    #[tokio::test]
    async fn a_directory_the_client_named_is_used_as_it_stands() {
        let (manager, _env) = manager();
        let chosen = test_env::unique_dir("srud-chosen");

        let id = manager
            .create(Some(chosen.clone()))
            .await
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

    #[tokio::test]
    async fn nothing_named_for_the_client_means_a_workspace() {
        let (manager, _env) = manager();
        let id = manager.create(Some(PathBuf::new())).await.expect("created");

        assert_given_a_workspace(&manager, &id, "an empty path names no directory");
    }

    #[tokio::test]
    async fn a_path_that_does_not_exist_means_a_workspace() {
        let (manager, _env) = manager();
        let gone = test_env::unique_dir("srud-gone").join("never-created");

        let id = manager.create(Some(gone)).await.expect("created");

        assert_given_a_workspace(&manager, &id, "a directory that is not there");
    }

    #[tokio::test]
    async fn a_file_where_a_directory_belongs_means_a_workspace() {
        let (manager, _env) = manager();
        let file = test_env::unique_dir("srud-file").join("a-file");
        std::fs::write(&file, "not a directory").expect("a file");

        let id = manager.create(Some(file)).await.expect("created");

        assert_given_a_workspace(&manager, &id, "a file is not a working directory");
    }

    #[tokio::test]
    async fn a_relative_path_means_a_workspace() {
        let (manager, _env) = manager();

        let id = manager
            .create(Some(PathBuf::from("relative/path")))
            .await
            .expect("created");

        assert_given_a_workspace(
            &manager,
            &id,
            "a relative path would resolve against the agent's own directory",
        );
    }

    #[tokio::test]
    async fn removing_a_session_removes_the_workspace_it_created() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        let cwd = manager.get(&id).expect("registered").cwd().to_path_buf();

        assert!(manager.remove(&id));

        assert!(!cwd.exists(), "{cwd:?} went with the session");
    }

    #[tokio::test]
    async fn removing_a_session_leaves_a_directory_the_client_named() {
        let (manager, _env) = manager();
        let chosen = test_env::unique_dir("srud-chosen");
        let id = manager.create(Some(chosen.clone())).await.expect("created");

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

    #[tokio::test]
    async fn remove_drops_the_session() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        assert!(manager.remove(&id));
        assert!(!manager.remove(&id), "a second removal finds nothing");
        assert!(manager.is_empty());
    }

    #[tokio::test]
    async fn a_new_session_has_no_title() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        assert_eq!(manager.title(&id), None);
        assert!(manager.list()[0].title.is_none());
    }

    #[tokio::test]
    async fn name_if_unnamed_only_the_first_caller_wins() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        assert!(manager.name_if_unnamed(&id, "first message").await.unwrap());
        // A later prompt must not clobber the name the session already has.
        assert!(!manager
            .name_if_unnamed(&id, "second message")
            .await
            .unwrap());
        assert_eq!(manager.title(&id).as_deref(), Some("first message"));
    }

    #[tokio::test]
    async fn set_title_overwrites_and_reports_unknown_ids() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        assert!(manager.name_if_unnamed(&id, "derived").await.unwrap());
        assert!(manager.set_title(&id, "chosen by the user").await.unwrap());
        assert_eq!(manager.title(&id).as_deref(), Some("chosen by the user"));
        assert!(!manager
            .set_title(&SessionId::new("nope"), "x")
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn a_blank_title_clears_the_name() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        manager.name_if_unnamed(&id, "named").await.unwrap();
        assert!(manager.set_title(&id, "   \n  ").await.unwrap());
        assert_eq!(manager.title(&id), None);
        // Cleared means unnamed, so the next prompt may name it again.
        assert!(manager.name_if_unnamed(&id, "named again").await.unwrap());
    }

    #[tokio::test]
    async fn titles_collapse_to_a_single_line() {
        let (manager, _env) = manager();
        let id = create(&manager).await;
        manager
            .set_title(&id, "  fix   the\n\n  timestamp  ")
            .await
            .unwrap();
        assert_eq!(manager.title(&id).as_deref(), Some("fix the timestamp"));
    }

    #[tokio::test]
    async fn list_reports_the_title() {
        let (manager, _env) = manager();
        let named = create(&manager).await;
        manager.set_title(&named, "Named session").await.unwrap();
        let listed = manager.list();
        let titled = listed
            .iter()
            .find(|info| info.session_id == named)
            .expect("the named session is listed");
        assert_eq!(titled.title.as_deref(), Some("Named session"));
    }

    #[tokio::test]
    async fn list_reports_the_working_directory_for_every_session() {
        let (manager, _env) = manager();
        let mut expected = [create(&manager).await, create(&manager).await];
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
