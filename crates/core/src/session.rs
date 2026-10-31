//! Session state: the recorded history plus the control block for the turn
//! that may be running.
//!
//! The history is the authority; everything else is derived from it. The turn
//! loop is the only writer, and the only other way history comes to exist is
//! [`Session::restored`] rebuilding it from a log — so recorded history and
//! model-visible context stay in agreement.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use tokio_util::sync::CancellationToken;

use crate::types::{ImagePart, ResponseItem, Role, SessionId, TurnInput};

/// The recorded conversation.
#[derive(Debug)]
pub struct SessionState {
    /// Every item the model has seen, in order.
    history: Vec<ResponseItem>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionState {
    /// Creates an empty state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
        }
    }

    /// Rebuilds state from a log's history.
    #[must_use]
    pub fn restored(history: Vec<ResponseItem>) -> Self {
        Self { history }
    }

    /// Reads the recorded history.
    #[must_use]
    pub fn history(&self) -> &[ResponseItem] {
        &self.history
    }

    /// Returns how many items are recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.history.len()
    }

    /// Returns whether nothing has been recorded yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.history.is_empty()
    }

    /// Appends one item to history.
    ///
    /// Crate-private: the turn loop is the only caller.
    pub(crate) fn push(&mut self, item: ResponseItem) {
        self.history.push(item);
    }

    /// Drops every item recorded at or after `mark`.
    ///
    /// Crate-private: only a discarded turn shortens history, and the log learns
    /// of it the same way it learns of a push.
    pub(crate) fn truncate(&mut self, mark: usize) {
        self.history.truncate(mark);
    }

    /// The last thing the user said, and where in the history it ends.
    ///
    /// Read when a session is rebuilt in a process that did not run its last
    /// turn: the mark a re-answer cuts at is not in the log, but the message and
    /// the images beside it still are. The mark sits after them, so cutting there
    /// takes the answer and leaves the question.
    ///
    /// A steer is recorded as a user message too, so a turn extended by one
    /// resolves to the steer. Re-answering starts a fresh turn from those words
    /// rather than resuming the turn they were said into.
    pub(crate) fn trailing_prompt(&self) -> Option<(usize, TurnInput)> {
        let start = self.history.iter().rposition(|item| {
            matches!(
                item,
                ResponseItem::Message {
                    role: Role::User,
                    ..
                }
            ) && !crate::context::is_item(item)
        })?;
        let ResponseItem::Message { content, .. } = &self.history[start] else {
            return None;
        };
        let mut end = start + 1;
        let mut images = Vec::new();
        while let Some(ResponseItem::Image { mime, data }) = self.history.get(end) {
            images.push(ImagePart {
                mime: mime.clone(),
                data: data.clone(),
            });
            end += 1;
        }
        Some((
            end,
            TurnInput {
                text: content.clone(),
                images,
            },
        ))
    }
}

/// A session: identity, the directory it works in, state, and the control block
/// for an in-flight turn.
#[derive(Debug)]
pub struct Session {
    id: SessionId,
    cwd: PathBuf,
    state: Mutex<SessionState>,
    active_turn: Mutex<Option<ActiveTurn>>,
    /// Where the last turn's input ends in the history.
    ///
    /// Recorded once the turn has written the message and the images it was
    /// asked with, because only then is the boundary known: the history gained
    /// an env-context block on the first turn, and steers waiting from the turn
    /// before are claimed ahead of the input. A later reader can find the input,
    /// but not where it stops and the answer starts.
    opening: Mutex<Option<usize>>,
    /// What the last turn was asked, as text and images.
    ///
    /// Kept for a turn that is asked again: the input is in the history, but the
    /// images beside it have no wire form to be read back from, and the text
    /// would have to be found by searching for the last user message.
    last_input: Mutex<Option<TurnInput>>,
    /// Steered messages waiting for the next boundary of the active turn.
    ///
    /// Separate from history on purpose: they are not part of the conversation
    /// until the turn records them, which is what keeps them out of a request
    /// that is already being built and puts them after whatever the current
    /// step produces.
    steers: Mutex<Vec<String>>,
}

/// Where the last turn's input ends, and what it was.
#[derive(Debug, Clone)]
pub struct OpenedTurn {
    /// The history length once the turn's input was recorded: the message it
    /// answered and the images sent with it. Everything at or after this point
    /// is the answer, which is what a discard takes back.
    pub mark: usize,
    /// What the turn was asked.
    pub input: TurnInput,
}

impl Session {
    /// Creates a session with a fresh id, working in `cwd`.
    #[must_use]
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self::with_id(SessionId::new(), cwd)
    }

    /// Creates a session with a specific id, for loading an existing one.
    #[must_use]
    pub fn with_id(id: SessionId, cwd: impl Into<PathBuf>) -> Self {
        Self {
            id,
            cwd: cwd.into(),
            state: Mutex::new(SessionState::new()),
            active_turn: Mutex::new(None),
            opening: Mutex::new(None),
            last_input: Mutex::new(None),
            steers: Mutex::new(Vec::new()),
        }
    }

    /// Rebuilds a session from the history its log holds.
    ///
    /// The turn loop is the only writer of history after this, so seeding here is
    /// the one other path — and it is what makes the log the authority rather than
    /// the process that wrote it. What is seeded is expected to be a history a
    /// model API accepts: closing a turn the log left open, and answering a call
    /// whose result never arrived, is [`crate::session_store::load`]'s job.
    ///
    /// The last thing the user said is picked up from the history, so the turn
    /// that answered it can be answered again in this process. Where its input
    /// ended is not in the log, but the message and the images beside it are.
    #[must_use]
    pub fn restored(id: SessionId, cwd: impl Into<PathBuf>, history: Vec<ResponseItem>) -> Self {
        let state = SessionState::restored(history);
        let prompt = state.trailing_prompt();
        let (opening, last_input) = match prompt {
            Some((mark, input)) => (Mutex::new(Some(mark)), Mutex::new(Some(input))),
            None => (Mutex::new(None), Mutex::new(None)),
        };
        Self {
            id,
            cwd: cwd.into(),
            state: Mutex::new(state),
            active_turn: Mutex::new(None),
            opening,
            last_input,
            steers: Mutex::new(Vec::new()),
        }
    }

    /// Returns this session's id.
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.id
    }

    /// Returns the directory this session works in.
    ///
    /// Relative paths in the model's requests resolve against it, and it is what
    /// the model is told about where it is.
    #[must_use]
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Locks and returns the state.
    ///
    /// # Panics
    ///
    /// Panics if the lock was poisoned by a panic in another thread holding it.
    pub fn state(&self) -> MutexGuard<'_, SessionState> {
        self.state.lock().expect("session state lock poisoned")
    }

    /// Claims the session for a new turn.
    ///
    /// Returns `None` if a turn is already active. A session runs at most one
    /// turn at a time, so this is the gate that enforces it.
    #[must_use]
    pub fn begin_turn(&self) -> Option<ActiveTurnGuard<'_>> {
        let mut slot = self.active_turn.lock().expect("active turn lock poisoned");
        if slot.is_some() {
            return None;
        }
        let cancellation = CancellationToken::new();
        slot.replace(ActiveTurn {
            cancellation: cancellation.clone(),
            closing: false,
        });
        Some(ActiveTurnGuard {
            session: self,
            cancellation,
        })
    }

    /// Requests cancellation of the active turn.
    ///
    /// Returns `false` if no turn is running. Cancellation is cooperative: this
    /// signals, it does not kill.
    pub fn interrupt(&self) -> bool {
        let slot = self.active_turn.lock().expect("active turn lock poisoned");
        match slot.as_ref() {
            Some(turn) => {
                turn.cancellation.cancel();
                true
            }
            None => false,
        }
    }

    /// Queues `text` for the active turn's next boundary, if one is running.
    ///
    /// Returns `false` when no turn is active or the turn has already decided to
    /// end — the caller is told the steer was refused and can retry it as a turn
    /// of its own. Acceptance only means the message will be recorded and
    /// answered by this turn: it becomes history when the loop claims it, after
    /// whatever the current step produces, so it never lands ahead of the answer
    /// it responds to. The caller announces it; the recording stays silent to
    /// avoid announcing it twice.
    ///
    /// The queue is pushed while the turn slot is held, so a turn closing in
    /// between cannot accept a message it will never read.
    pub fn steer(&self, text: &str) -> bool {
        let slot = self.active_turn.lock().expect("active turn lock poisoned");
        let Some(turn) = slot.as_ref() else {
            return false;
        };
        if turn.closing {
            return false;
        }
        self.steers
            .lock()
            .expect("steer queue lock poisoned")
            .push(text.to_owned());
        true
    }

    /// Takes every steered message waiting for the next boundary.
    #[must_use]
    pub fn take_steers(&self) -> Vec<String> {
        std::mem::take(&mut *self.steers.lock().expect("steer queue lock poisoned"))
    }

    /// Puts messages a boundary could not record back at the head of the queue.
    ///
    /// Head, not tail: they were said before anything that arrived while the
    /// failure was being handled, and the order they were said in is the order
    /// the model must see them.
    pub fn requeue_steers(&self, texts: Vec<String>) {
        let mut queue = self.steers.lock().expect("steer queue lock poisoned");
        queue.splice(0..0, texts);
    }

    /// Marks the active turn as closed to steering.
    ///
    /// Returns `false` when steers are waiting, which tells the loop the turn may
    /// not end yet: they are answered first. Returns `true` once the turn is
    /// sealed. Both the check and the seal happen under the turn slot, so a steer
    /// either lands before it and is answered, or is refused after it — there is
    /// no state where an accepted message has no one to answer it.
    #[must_use]
    pub fn seal_turn(&self) -> bool {
        let mut slot = self.active_turn.lock().expect("active turn lock poisoned");
        if !self
            .steers
            .lock()
            .expect("steer queue lock poisoned")
            .is_empty()
        {
            return false;
        }
        if let Some(turn) = slot.as_mut() {
            turn.closing = true;
        }
        true
    }

    /// Returns whether a turn is currently running.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.active_turn
            .lock()
            .expect("active turn lock poisoned")
            .is_some()
    }

    /// Records where the active turn's input ended, and what it was.
    ///
    /// Called by the turn loop once the history holds the input itself: the
    /// message and the images beside it are what a re-answer keeps, so the mark
    /// has to sit after them rather than before.
    pub(crate) fn note_opening(&self, mark: usize, input: &TurnInput) {
        *self.opening.lock().expect("opening lock poisoned") = Some(mark);
        *self.last_input.lock().expect("input lock poisoned") = Some(input.clone());
    }

    /// The last turn's input and where it ends, for asking the same thing again.
    ///
    /// `None` for a session that has run no turn, or whose history holds no user
    /// message. A session rebuilt from a log reads both from the history it was
    /// seeded with — see [`Session::restored`].
    #[must_use]
    pub fn last_prompt(&self) -> Option<OpenedTurn> {
        let mark = (*self.opening.lock().expect("opening lock poisoned"))?;
        let input = self
            .last_input
            .lock()
            .expect("input lock poisoned")
            .clone()?;
        Some(OpenedTurn { mark, input })
    }

    /// Cuts history back to `mark`, dropping everything recorded after it.
    ///
    /// `mark` is a [`SessionState::len`] taken once the turn's input was
    /// recorded, so the question it answered stays and only the answer goes.
    /// A mark at or past the end drops nothing, so asking twice costs nothing.
    ///
    /// Returns whether anything was dropped. A turn whose output was cut has to
    /// be recorded as discarded; one that contributed nothing present has nothing
    /// to record.
    pub fn truncate(&self, mark: usize) -> bool {
        let mut state = self.state();
        if mark >= state.len() {
            return false;
        }
        state.truncate(mark);
        true
    }
}

/// Runtime control for the turn that is currently running.
#[derive(Debug)]
pub struct ActiveTurn {
    /// Signalled to request cooperative cancellation.
    cancellation: CancellationToken,
    /// Set once the turn has decided to end: later steers are refused rather
    /// than accepted into a turn that will never read them.
    closing: bool,
}

/// Holds the session's turn slot and releases it on drop.
#[derive(Debug)]
pub struct ActiveTurnGuard<'a> {
    session: &'a Session,
    cancellation: CancellationToken,
}

impl ActiveTurnGuard<'_> {
    /// Returns the cancellation token for this turn.
    #[must_use]
    pub fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

impl Drop for ActiveTurnGuard<'_> {
    fn drop(&mut self) {
        let mut slot = self
            .session
            .active_turn
            .lock()
            .expect("active turn lock poisoned");
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session working in an arbitrary directory; these tests are about the
    /// turn slot, not about where the work happens.
    fn session() -> Session {
        Session::new(PathBuf::from("workspace"))
    }

    #[test]
    fn session_starts_empty() {
        let session = session();
        assert!(session.state().is_empty());
        assert!(!session.is_busy());
    }

    #[test]
    fn the_working_directory_is_readable() {
        let cwd = std::env::temp_dir().join("srud-session-ws");
        let session = Session::new(cwd.clone());
        assert_eq!(session.cwd(), cwd.as_path());
    }

    #[test]
    fn a_steer_is_refused_once_the_turn_has_sealed() {
        let session = session();
        assert!(!session.steer("nothing to steer into"), "no turn, no steer");

        let turn = session.begin_turn();
        assert!(session.steer("one"));
        assert!(
            !session.seal_turn(),
            "a waiting steer keeps the turn open: it has not been answered"
        );
        assert_eq!(session.take_steers(), vec!["one".to_string()]);

        assert!(session.seal_turn(), "an empty queue lets the turn end");
        assert!(
            !session.steer("two"),
            "a sealed turn refuses instead of swallowing"
        );

        drop(turn);
        assert!(!session.steer("three"), "a finished turn refuses too");
    }

    #[test]
    fn requeued_steers_stay_ahead_of_newer_ones() {
        let session = session();
        let turn = session.begin_turn();
        assert!(session.steer("newer"));
        session.requeue_steers(vec!["older".into()]);
        assert_eq!(
            session.take_steers(),
            vec!["older".to_string(), "newer".to_string()]
        );
        drop(turn);
    }

    #[test]
    fn only_one_turn_can_be_active() {
        let session = session();
        let first = session.begin_turn();
        assert!(first.is_some());
        assert!(session.is_busy());

        let second = session.begin_turn();
        assert!(second.is_none(), "a second concurrent turn must be refused");

        drop(first);
        assert!(!session.is_busy());

        let third = session.begin_turn();
        assert!(third.is_some(), "the slot is reusable after release");
    }

    #[test]
    fn dropping_the_guard_always_releases_the_slot() {
        let session = session();
        {
            let _guard = session.begin_turn().expect("slot is free");
            assert!(session.is_busy());
        }
        assert!(!session.is_busy());
    }

    #[test]
    fn interrupt_signals_the_active_turn() {
        let session = session();
        assert!(!session.interrupt(), "no turn to interrupt");

        let guard = session.begin_turn().expect("slot is free");
        let token = guard.cancellation();
        assert!(!token.is_cancelled());

        assert!(session.interrupt());
        assert!(token.is_cancelled(), "the token is signalled");
    }

    #[test]
    fn history_records_in_order() {
        let session = session();
        {
            let mut state = session.state();
            state.push(ResponseItem::user("one"));
            state.push(ResponseItem::assistant("two"));
        }
        assert_eq!(session.state().len(), 2);
    }
}
