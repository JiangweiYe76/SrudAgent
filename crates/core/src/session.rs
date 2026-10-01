//! Session state: the recorded history plus the control block for the turn
//! that may be running.
//!
//! The history is the authority; everything else is derived from it. The turn
//! loop is the only writer, so recorded history and model-visible context stay
//! in agreement.

use std::sync::{Mutex, MutexGuard};

use tokio_util::sync::CancellationToken;

use crate::types::{ResponseItem, SessionId};

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
}

/// A session: identity, state, and the control block for an in-flight turn.
#[derive(Debug)]
pub struct Session {
    id: SessionId,
    state: Mutex<SessionState>,
    active_turn: Mutex<Option<ActiveTurn>>,
}

impl Session {
    /// Creates a session with a fresh id.
    #[must_use]
    pub fn new() -> Self {
        Self::with_id(SessionId::new())
    }

    /// Creates a session with a specific id, for loading an existing one.
    #[must_use]
    pub fn with_id(id: SessionId) -> Self {
        Self {
            id,
            state: Mutex::new(SessionState::new()),
            active_turn: Mutex::new(None),
        }
    }

    /// Returns this session's id.
    #[must_use]
    pub fn id(&self) -> SessionId {
        self.id
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

    /// Returns whether a turn is currently running.
    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.active_turn
            .lock()
            .expect("active turn lock poisoned")
            .is_some()
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

/// Runtime control for the turn that is currently running.
#[derive(Debug)]
pub struct ActiveTurn {
    /// Signalled to request cooperative cancellation.
    cancellation: CancellationToken,
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

    #[test]
    fn session_starts_empty() {
        let session = Session::new();
        assert!(session.state().is_empty());
        assert!(!session.is_busy());
    }

    #[test]
    fn only_one_turn_can_be_active() {
        let session = Session::new();
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
        let session = Session::new();
        {
            let _guard = session.begin_turn().expect("slot is free");
            assert!(session.is_busy());
        }
        assert!(!session.is_busy());
    }

    #[test]
    fn interrupt_signals_the_active_turn() {
        let session = Session::new();
        assert!(!session.interrupt(), "no turn to interrupt");

        let guard = session.begin_turn().expect("slot is free");
        let token = guard.cancellation();
        assert!(!token.is_cancelled());

        assert!(session.interrupt());
        assert!(token.is_cancelled(), "the token is signalled");
    }

    #[test]
    fn history_records_in_order() {
        let session = Session::new();
        {
            let mut state = session.state();
            state.push(ResponseItem::user("one"));
            state.push(ResponseItem::assistant("two"));
        }
        assert_eq!(session.state().len(), 2);
    }
}
