//! Bridges the core runtime's synchronous [`EventSink`] onto an async
//! broadcast stream of ACP notifications.
//!
//! The turn loop emits events through an `EventSink`, which is a synchronous
//! trait that must not block. This sink forwards each event into a
//! `tokio::sync::broadcast` channel already encoded as a
//! `session/update` notification, so a host can `subscribe` and drain the
//! stream without touching the runtime.
//!
//! The channel carries session-level notifications too — a title change, for
//! one — which no turn produces and which therefore go out through
//! [`EventHub::send`] rather than a sink.
//!
//! The turn id is not known when a turn is started — the runtime generates it
//! and reports it in the first `TurnStarted` event. The sink captures it there
//! and stamps it onto every later notification of the same turn under
//! `_meta.srud.turnId`. An event emitted before that — a steer announcement
//! arriving between the turn starting and its id being reported — is held
//! until the id arrives rather than dropped, since it belongs to the turn all
//! the same and cannot be re-announced later.
//!
//! Every notification also carries the session's next sequence number under
//! `_meta.srud.seq`. Updates are increments, so a dropped one costs text, not
//! just rendering granularity: the number is what lets a client notice that the
//! stream skipped something instead of reading a shortened conversation as
//! complete.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;

use srud_core::types::{Event, EventSink, SessionId as CoreSessionId, TurnId};
use srud_protocol::acp::{Notification, SessionId, SessionNotification};
use srud_protocol::srud::meta::SrudMeta;
use tokio::sync::broadcast;

use crate::convert::{to_notification, ToolDiffIndex};

/// The channel capacity for buffered notifications.
///
/// A slow subscriber that falls behind loses the oldest notifications rather
/// than blocking the turn — `broadcast` reports `Lagged`, and the host decides
/// whether to reconcile. Message deltas are increments, so a dropped chunk is
/// text the subscriber never sees; `_meta.srud.seq` is how it detects that.
const CHANNEL_CAPACITY: usize = 512;

/// The per-session sequence numbers every notification is stamped with.
///
/// The hub and the turn sinks both write to one session's stream, so they draw
/// from one counter: a title sent from outside a turn and the turn's own updates
/// must not hand out the same number. Cloning shares the count.
#[derive(Clone, Default)]
struct SeqCounter {
    next: Arc<Mutex<HashMap<SessionId, u64>>>,
}

impl SeqCounter {
    fn take(&self, session: &SessionId) -> u64 {
        let mut next = self.next.lock().expect("sequence counter lock poisoned");
        let seq = next.get(session).copied().unwrap_or(0).saturating_add(1);
        next.insert(session.clone(), seq);
        seq
    }

    /// Adds the session's next number to a notification's `_meta.srud`.
    ///
    /// Any other keys the notification's `_meta` carries are preserved.
    fn stamp(&self, notification: &mut Notification<SessionNotification>) {
        let Some(params) = notification.params.as_mut() else {
            return;
        };
        let seq = self.take(&params.session_id);
        let mut meta = params.meta.take().unwrap_or_default();
        let mut srud = SrudMeta::from_meta(&meta).unwrap_or_default();
        srud.seq = Some(seq);
        // Our own notifications never occupy a reserved trace key, so this
        // cannot fail.
        let _ = srud.insert_into(&mut meta);
        params.meta = Some(meta);
    }
}

/// A cloneable hub that fans core events out to ACP notification subscribers.
#[derive(Clone)]
pub struct EventHub {
    tx: broadcast::Sender<Notification<SessionNotification>>,
    seq: SeqCounter,
}

impl EventHub {
    /// Creates an empty hub.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(CHANNEL_CAPACITY);
        Self {
            tx,
            seq: SeqCounter::default(),
        }
    }

    /// Subscribes to the notification stream.
    ///
    /// Only notifications emitted after this call are received.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Notification<SessionNotification>> {
        self.tx.subscribe()
    }

    /// Broadcasts a notification that did not come from a turn.
    ///
    /// Session-level changes such as a new title are not runtime events, so
    /// they have no [`EventSink`] to travel through and are sent directly.
    pub fn send(&self, mut notification: Notification<SessionNotification>) {
        self.seq.stamp(&mut notification);
        self.publish(notification);
    }

    /// Stamps the next sequence number and broadcasts, returning what went out.
    ///
    /// A caller that also hands the conversation over on a response — `session/load`
    /// does — uses the returned copy so the response and the stream agree on both
    /// content and numbering.
    pub(crate) fn send_replayed(
        &self,
        mut notification: Notification<SessionNotification>,
    ) -> Option<SessionNotification> {
        self.seq.stamp(&mut notification);
        let params = notification.params.clone();
        self.publish(notification);
        params
    }

    /// Sends a notification as it stands, without assigning it a number.
    fn publish(&self, notification: Notification<SessionNotification>) {
        // A send error only means there are no subscribers right now; a missing
        // listener must never fail the request that caused the notification.
        let _ = self.tx.send(notification);
    }

    /// Binds a sink for one session's turns.
    ///
    /// `context_size` is the configured window of the session's model, carried
    /// on every usage notification the sink emits; `None` when the model has
    /// no window configured.
    #[must_use]
    pub fn sink_for(&self, session_id: CoreSessionId, context_size: Option<u64>) -> TurnSink {
        TurnSink {
            tx: self.tx.clone(),
            seq: self.seq.clone(),
            session_id: SessionId::new(session_id.to_string()),
            turn_id: Arc::new(OnceLock::new()),
            context_size,
            pending: Arc::new(Mutex::new(Vec::new())),
            diffs: Arc::new(Mutex::new(ToolDiffIndex::default())),
        }
    }
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

/// An [`EventSink`] that converts and broadcasts one session's events.
#[derive(Clone)]
pub struct TurnSink {
    tx: broadcast::Sender<Notification<SessionNotification>>,
    /// The session's counter, shared with the hub and with the sink of every
    /// turn the session runs.
    seq: SeqCounter,
    session_id: SessionId,
    turn_id: Arc<OnceLock<TurnId>>,
    context_size: Option<u64>,
    /// Events emitted before the turn reported its id, in the order they came.
    ///
    /// A clone shares this with the sink the turn runs on, so an announcement
    /// made from outside the turn — a steer — is kept until `TurnStarted`
    /// names the turn, then sent under that id. Only events that cannot be
    /// re-announced belong here, and there are few of them: everything the
    /// turn itself emits follows its id.
    pending: Arc<Mutex<Vec<Event>>>,
    /// Tool-call diffs waiting to ride along to their result messages.
    ///
    /// Shared across clones, as the sink is cloned into `active_sinks` while
    /// the turn runs on the original: cloning must not fork the map a call
    /// stores its diffs into and later takes them back from.
    diffs: Arc<Mutex<ToolDiffIndex>>,
}

impl TurnSink {
    /// Converts and broadcasts one event under a known turn id.
    ///
    /// The id is the one the notification is filed under, which is not always
    /// the turn the event is about: a discard names the turn it takes back, and
    /// is sent before that turn's replacement starts.
    pub(crate) fn broadcast(&self, turn_id: TurnId, event: Event) {
        let mut diffs = self.diffs.lock().expect("tool diff index lock poisoned");
        let Some(mut notification) = to_notification(
            &self.session_id,
            &turn_id.to_string(),
            event,
            self.context_size,
            &mut diffs,
        ) else {
            return;
        };
        self.seq.stamp(&mut notification);
        // A send error only means there are no subscribers right now; the
        // turn must never block or fail because nobody is listening.
        let _ = self.tx.send(notification);
    }
}

impl EventSink for TurnSink {
    fn emit(&self, event: Event) {
        if let Event::TurnStarted { turn_id } = event {
            let _ = self.turn_id.set(turn_id);
            // Held events were emitted before this one, so they go out first.
            let held =
                std::mem::take(&mut *self.pending.lock().expect("pending sink lock poisoned"));
            for earlier in held {
                self.broadcast(turn_id, earlier);
            }
            // A turn start has no wire form; it only names the turn.
            return;
        }
        match self.turn_id.get() {
            Some(turn_id) => self.broadcast(*turn_id, event),
            None => self
                .pending
                .lock()
                .expect("pending sink lock poisoned")
                .push(event),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srud_core::types::TurnEndReason;
    use srud_protocol::acp::{SessionInfoUpdate, SessionUpdate, CLIENT_METHOD_NAMES};

    /// A `session_info_update` notification, as a title change produces.
    fn title_notification(title: &str) -> Notification<SessionNotification> {
        Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                SessionId::new("s1"),
                SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title(title)),
            )),
        }
    }

    #[test]
    fn sink_broadcasts_mapped_notifications() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let sink = hub.sink_for(CoreSessionId::new(), None);

        let turn_id = TurnId::new();
        sink.emit(Event::TurnStarted { turn_id });
        sink.emit(Event::AgentMessageDelta {
            message_id: None,
            delta: "hi".into(),
        });
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
    fn an_event_emitted_before_turn_started_is_sent_under_its_id() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let sink = hub.sink_for(CoreSessionId::new(), None);

        // A steer can be announced from outside the turn while the turn is
        // still opening, before the runtime has reported its id.
        sink.emit(Event::UserMessage {
            content: "steer".into(),
        });
        assert!(
            rx.try_recv().is_err(),
            "nothing goes out before the turn has an id"
        );

        let turn_id = TurnId::new();
        sink.emit(Event::TurnStarted { turn_id });

        let received = rx.try_recv().expect("the held message is sent once named");
        assert!(matches!(
            received.params.as_ref().unwrap().update,
            SessionUpdate::UserMessageChunk(_)
        ));
        let value = serde_json::to_value(&received).unwrap();
        assert_eq!(
            value["params"]["_meta"]["srud"]["turnId"],
            serde_json::json!(turn_id.to_string()),
            "the held event carries the id it waited for"
        );
    }

    #[test]
    fn notification_carries_turn_id_in_meta() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let sink = hub.sink_for(CoreSessionId::new(), None);

        let turn_id = TurnId::new();
        sink.emit(Event::TurnStarted { turn_id });
        sink.emit(Event::AgentMessageDelta {
            message_id: None,
            delta: "x".into(),
        });

        let received = rx.try_recv().expect("one notification");
        let value = serde_json::to_value(&received).unwrap();
        assert_eq!(
            value["params"]["_meta"]["srud"]["turnId"],
            serde_json::json!(turn_id.to_string())
        );
    }

    #[test]
    fn one_session_is_numbered_in_order_across_sink_and_hub() {
        // A title sent from outside a turn and a turn's own updates are one
        // stream; they must draw from one counter or a client reads a gap
        // between two updates that were never dropped.
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let core_session = CoreSessionId::new();
        let sink = hub.sink_for(core_session, None);
        let title = Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                SessionId::new(core_session.to_string()),
                SessionUpdate::SessionInfoUpdate(SessionInfoUpdate::new().title("named")),
            )),
        };

        let turn_id = TurnId::new();
        sink.emit(Event::TurnStarted { turn_id });
        sink.emit(Event::AgentMessageDelta {
            message_id: None,
            delta: "x".into(),
        });
        hub.send(title);

        let seqs: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|notification| {
                let value = serde_json::to_value(&notification).unwrap();
                value["params"]["_meta"]["srud"]["seq"]
                    .as_u64()
                    .expect("every notification is numbered")
            })
            .collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn each_session_is_numbered_from_its_own_start() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        let first = hub.sink_for(CoreSessionId::new(), None);
        let second = hub.sink_for(CoreSessionId::new(), None);

        for sink in [&first, &second] {
            sink.emit(Event::TurnStarted {
                turn_id: TurnId::new(),
            });
            sink.emit(Event::AgentMessageDelta {
                message_id: None,
                delta: "x".into(),
            });
        }

        let seqs: Vec<u64> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|notification| {
                let value = serde_json::to_value(&notification).unwrap();
                value["params"]["_meta"]["srud"]["seq"]
                    .as_u64()
                    .expect("every notification is numbered")
            })
            .collect();
        assert_eq!(seqs, vec![1, 1], "a session's numbers are only its own");
    }

    #[test]
    fn a_direct_notification_reaches_subscribers() {
        let hub = EventHub::new();
        let mut rx = hub.subscribe();
        hub.send(title_notification("named"));

        let received = rx.try_recv().expect("the notification is broadcast");
        let value = serde_json::to_value(&received).unwrap();
        assert_eq!(
            value["params"]["update"]["sessionUpdate"],
            "session_info_update"
        );
        assert_eq!(value["params"]["update"]["title"], "named");
    }

    #[test]
    fn sending_without_subscribers_does_not_panic() {
        let hub = EventHub::new();
        hub.send(title_notification("named"));
    }

    #[test]
    fn sink_without_subscribers_does_not_panic() {
        let hub = EventHub::new();
        let sink = hub.sink_for(CoreSessionId::new(), None);
        sink.emit(Event::TurnStarted {
            turn_id: TurnId::new(),
        });
        sink.emit(Event::AgentMessageDelta {
            message_id: None,
            delta: "x".into(),
        });
    }
}
