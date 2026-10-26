// What a steer does to the turn it is steered into.
//
// A steer is a durable part of the conversation, which fixes three properties:
// it must be *recorded* to the log rather than held in memory alone; it must be
// *claimed at a boundary*, so it enters the next request after the answer it
// responds to rather than ahead of it; and it must be *answered* — a turn that
// holds an unread steer does not end over it. A steer left over from a turn that
// died first was said before the next input, so it belongs ahead of that input
// in the history.
use std::sync::Mutex;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use srud_core::client::{
    ModelClient, ModelError, ModelEvent, ModelRequest, ModelRequestItem, ModelStream,
};
use srud_core::run_turn;
use srud_core::session::Session;
use srud_core::session_event::SessionEvent;
use srud_core::session_log::Volatile;
use srud_core::tools::ToolRegistry;
use srud_core::types::{Event, EventSink, ResponseItem, Role, TurnEndReason, TurnInput};

/// The user texts of a request, in the order the model reads them.
fn users_of(request: &ModelRequest) -> Vec<String> {
    request
        .items
        .iter()
        .filter_map(|item| match item {
            ModelRequestItem::Message {
                role: Role::User,
                content,
            } => Some(content.clone()),
            _ => None,
        })
        .collect()
}

/// The user texts of a session's history, in order, without the env-context
/// block — that is a user-role item too, but it is context rather than
/// something anyone said.
fn users_in(session: &Session) -> Vec<String> {
    let state = session.state();
    state
        .history()
        .iter()
        .filter_map(|item| match item {
            ResponseItem::Message {
                role: Role::User,
                content,
            } => {
                let message = ResponseItem::Message {
                    role: Role::User,
                    content: content.clone(),
                };
                (!srud_core::context::is_item(&message)).then(|| content.clone())
            }
            _ => None,
        })
        .collect()
}

/// The user texts a log holds, in order, again without the env-context block.
fn users_in_log(log: &Volatile) -> Vec<String> {
    log.records()
        .into_iter()
        .filter_map(|logged| match logged.event {
            SessionEvent::Item {
                item:
                    ResponseItem::Message {
                        role: Role::User,
                        content,
                    },
                ..
            } => {
                let message = ResponseItem::Message {
                    role: Role::User,
                    content: content.clone(),
                };
                (!srud_core::context::is_item(&message)).then(|| content.clone())
            }
            _ => None,
        })
        .collect()
}

/// A registry with nothing in it; these tests are about messages, not tools.
fn tools() -> ToolRegistry {
    ToolRegistry::new()
}

/// A sink that keeps what it was told, to count announcements.
#[derive(Default)]
struct Heard(Mutex<Vec<Event>>);

impl EventSink for Heard {
    fn emit(&self, event: Event) {
        self.0.lock().expect("heard lock").push(event);
    }
}

impl Heard {
    /// How many times a user message was announced on the wire.
    fn user_announcements(&self) -> usize {
        self.0
            .lock()
            .expect("heard lock")
            .iter()
            .filter(|event| matches!(event, Event::UserMessage { .. }))
            .count()
    }
}

/// A model that injects a steer while its first request is in flight — the way
/// a user clicks "send now" during a streaming answer — then answers from a
/// script. Every request's user texts are kept so a test can say what the model
/// was asked, and in what order.
struct SteersWhileAnswering<'a> {
    session: &'a Session,
    /// The message injected during the first request, if any.
    inject: Mutex<Option<String>>,
    /// One round of events per request; a round missing means the model refuses.
    rounds: Mutex<Vec<Vec<ModelEvent>>>,
    seen: Mutex<Vec<Vec<String>>>,
}

impl<'a> SteersWhileAnswering<'a> {
    fn new(session: &'a Session, inject: Option<String>, rounds: Vec<Vec<ModelEvent>>) -> Self {
        Self {
            session,
            inject: Mutex::new(inject),
            rounds: Mutex::new(rounds),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The user texts each request carried.
    fn seen(&self) -> Vec<Vec<String>> {
        self.seen.lock().expect("seen lock").clone()
    }
}

#[async_trait]
impl ModelClient for SteersWhileAnswering<'_> {
    async fn stream(
        &self,
        request: ModelRequest,
        _cancel: CancellationToken,
    ) -> Result<ModelStream, ModelError> {
        self.seen
            .lock()
            .expect("seen lock")
            .push(users_of(&request));
        if let Some(text) = self.inject.lock().expect("inject lock").take() {
            assert!(
                self.session.steer(&text),
                "the turn is running, so the steer must be accepted"
            );
        }
        let mut rounds = self.rounds.lock().expect("rounds lock");
        let Some(events) = rounds.first().cloned() else {
            return Err(ModelError::Rejected("script exhausted".into()));
        };
        rounds.remove(0);
        let events: Vec<Result<ModelEvent, ModelError>> = events.into_iter().map(Ok).collect();
        Ok(Box::pin(futures::stream::iter(events)))
    }
}

#[tokio::test]
async fn a_steer_is_recorded_at_a_boundary_and_answered_in_the_same_turn() {
    let log = Volatile::new();
    let heard = Heard::default();
    let session = Session::new("workspace");
    let model = SteersWhileAnswering::new(
        &session,
        Some("wait, do this too".into()),
        vec![
            vec![
                ModelEvent::TextDelta {
                    delta: "on it".into(),
                },
                ModelEvent::Done,
            ],
            vec![
                ModelEvent::TextDelta {
                    delta: "done".into(),
                },
                ModelEvent::Done,
            ],
        ],
    );

    let result = run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        &model,
        &tools(),
        &log,
        &heard,
    )
    .await
    .expect("the turn runs");
    assert_eq!(
        result.reason,
        TurnEndReason::Completed,
        "the turn does not end over a message nobody answered"
    );

    // The steer arrived mid-answer, so the first request cannot carry it — and
    // the second must, *after* what it responds to rather than before.
    let seen = model.seen();
    assert_eq!(seen.len(), 2, "the steer bought a second model request");
    assert!(
        seen[0]
            .iter()
            .all(|text| !text.contains("wait, do this too")),
        "the first request was built before the steer arrived: {:?}",
        seen[0]
    );
    let answered = &seen[1];
    let input_at = answered
        .iter()
        .position(|text| text.contains("hi"))
        .expect("the turn's own input is still there");
    let steer_at = answered
        .iter()
        .position(|text| text.contains("wait, do this too"))
        .expect("the steer reaches the model");
    assert!(
        input_at < steer_at,
        "the steer must follow the answer it responds to: {:?}",
        answered
    );

    // And it is a real part of the conversation: history and log agree, in order.
    assert_eq!(
        users_in(&session),
        vec!["hi".to_string(), "wait, do this too".to_string()]
    );
    assert_eq!(
        users_in_log(&log),
        vec!["hi".to_string(), "wait, do this too".to_string()],
        "a steer lives in the log, not only in memory"
    );

    // Announced once, when it was accepted: the boundary claims it silently.
    assert_eq!(
        heard.user_announcements(),
        1,
        "the turn's own input is the only user message announced by the turn"
    );

    // Once the turn is over there is nothing to steer into, and the answer is
    // a refusal — not silence.
    assert!(
        !session.steer("too late"),
        "a finished turn refuses rather than swallowing the message"
    );
}

#[tokio::test]
async fn a_steer_a_dead_turn_never_read_is_answered_before_the_next_input() {
    let log = Volatile::new();
    let heard = Heard::default();
    let session = Session::new("workspace");

    // The model steers and then fails, so the turn dies with the steer still
    // waiting: nobody read it, and it is not discarded for that.
    let dying = SteersWhileAnswering::new(&session, Some("left waiting".into()), vec![]);
    let first = run_turn(
        &session,
        TurnInput {
            text: "hi".into(),
            images: Vec::new(),
        },
        &dying,
        &tools(),
        &log,
        &heard,
    )
    .await
    .expect("a failing model still completes the turn");
    assert_eq!(first.reason, TurnEndReason::Error);
    assert_eq!(
        users_in(&session),
        vec!["hi".to_string()],
        "the steer waits instead of entering history unrecorded"
    );

    // The next turn claims it before its own input: it was said first.
    let answering = SteersWhileAnswering::new(&session, None, vec![vec![ModelEvent::Done]]);
    run_turn(
        &session,
        TurnInput {
            text: "second question".into(),
            images: Vec::new(),
        },
        &answering,
        &tools(),
        &log,
        &heard,
    )
    .await
    .expect("the turn runs");

    assert_eq!(
        users_in(&session),
        vec![
            "hi".to_string(),
            "left waiting".to_string(),
            "second question".to_string()
        ]
    );
    assert_eq!(
        users_in_log(&log),
        vec![
            "hi".to_string(),
            "left waiting".to_string(),
            "second question".to_string()
        ],
        "the log carries the same order the model was shown"
    );
    let asked = answering.seen();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0]
            .iter()
            .filter(|text| text.contains("left waiting"))
            .count(),
        1,
        "the second request carries the steer exactly once"
    );
}
