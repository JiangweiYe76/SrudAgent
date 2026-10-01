//! Bridges the core runtime's synchronous [`EventSink`] onto an async
//! broadcast stream of ACP notifications.
//!
//! The turn loop emits events through an `EventSink`, which is a synchronous
//! trait that must not block. This sink forwards each event into a
//! `tokio::sync::broadcast` channel already encoded as a
//! `session/update` notification, so a host can `subscribe` and drain the
//! stream without touching the runtime.
//!
//! The turn id is not known when a turn is started — the runtime generates it
//! and reports it in the first `TurnStarted` event. The sink captures it there
//! and stamps it onto every later notification of the same turn under
//! `_meta.srud.turnId`.

use std::sync::OnceLock;

use srud_core::types::{Event, EventSink, SessionId as CoreSessionId, TurnId};
use srud_protocol::acp::{Notification, SessionId, SessionNotification};
use tokio::sync::broadcast;

use crate::convert::to_notification;

/// The channel capacity for buffered notifications.
///
/// A slow subscriber that falls behind loses the oldest notifications rather
/// than blocking the turn — `broadcast` reports `Lagged`, and the host decides
/// whether to reconcile. Chunk-level message deltas are accumulative, so a
/// dropped chunk only costs rendering granularity, not correctness.
const CHANNEL_CAPACITY: usize = 512;

/// A cloneable hub that fans core events out to ACP notification subscribers.
#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<Notification<SessionNotification>>,
}

impl EventHub {
    /// Creates an empty hub.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self { tx }
    }

    /// Subscribes to the notification stream.
    ///
    /// Only notifications emitted after this call are received.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Notification<SessionNotification>> {
        self.tx.subscribe()
    }

    /// Binds a sink for one session's turns.
    #[must_use]
    pub fn sink_for(&self, session_id: CoreSessionId) -> TurnSink {
        TurnSink {
            tx: self.tx.clone(),
            session_id: SessionId::new(session_id.to_string()),
            turn_id: OnceLock::new(),
        }
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

/// An [`EventSink`] that converts and broadcasts one session's events.
pub struct TurnSink {
    tx: broadcast::Sender<Notification<SessionNotification>>,
    session_id: SessionId,
    turn_id: OnceLock<TurnId>,
}

impl EventSink for TurnSink {
    fn emit(&self, event: Event) {
        let turn_id = match &event {
            Event::TurnStarted { turn_id } => {
                let _ = self.turn_id.set(*turn_id);
                *turn_id
            }
            _ => match self.turn_id.get() {
                Some(id) => *id,
                // An event before TurnStarted cannot be attributed to a turn;
                // the runtime never produces this ordering.
                None => return,
            },
        };
        let Some(notification) = to_notification(&self.session_id, &turn_id.to_string(), event)
        else {
            return;
        };
        // A send error only means there are no subscribers right now; the
        // turn must never block or fail because nobody is listening.
        let _ = self.tx.send(notification);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srud_core::types::TurnEndReason;

    #[test]
    fn sink_broadcasts_mapped_notifications() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let sink = hub.sink_for(CoreSessionId::new());

        sink.emit(Event::AgentMessageDelta { delta: "hi".into() });
        assert!(
            rx.try_recv().is_err(),
            "events before TurnStarted are not attributable"
        );

        let turn_id = TurnId::new();
        sink.emit(Event::TurnStarted { turn_id });
        sink.emit(Event::AgentMessageDelta { delta: "hi".into() });
        sink.emit(Event::TurnComplete {
            turn_id,
            reason: TurnEndReason::Completed,
        });

        let received = rx.try_recv().expect("the delta reaches the subscriber");
        assert_eq!(received.method.as_ref(), "session/update");
        assert!(
            rx.try_recv().is_err(),
            "turn start/complete are not broadcast"
        );
    }

    #[test]
    fn notification_carries_turn_id_in_meta() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let sink = hub.sink_for(CoreSessionId::new());

        let turn_id = TurnId::new();
        sink.emit(Event::TurnStarted { turn_id });
        sink.emit(Event::AgentMessageDelta { delta: "x".into() });

        let received = rx.try_recv().expect("one notification");
        let value = serde_json::to_value(&received).unwrap();
        assert_eq!(
            value["params"]["_meta"]["srud"]["turnId"],
            serde_json::json!(turn_id.to_string())
        );
    }

    #[test]
    fn sink_without_subscribers_does_not_panic() {
        let hub = EventHub::new();
        let sink = hub.sink_for(CoreSessionId::new());
        sink.emit(Event::TurnStarted {
            turn_id: TurnId::new(),
        });
        sink.emit(Event::AgentMessageDelta { delta: "x".into() });
    }
}
