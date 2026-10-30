//! The ACP agent: JSON-RPC dispatch over the core runtime.
//!
//! [`Agent`] is the whole protocol surface. A host feeds it `rpc_request`
//! envelopes through [`Agent::handle`] and gets the JSON-RPC reply back, and
//! subscribes with [`Agent::subscribe`] for the `session/update` stream. It
//! owns no socket and no connection state beyond the `initialize` gate.
//!
//! Method coverage:
//!
//! | Method            | Behaviour                                            |
//! |-------------------|------------------------------------------------------|
//! | `initialize`      | version + capability negotiation (must come first)   |
//! | `session/new`     | register an in-memory session                        |
//! | `session/load`    | rebuild a session from its log and replay it         |
//! | `session/resume`  | rebuild a session from its log, without replaying    |
//! | `session/prompt`  | run one turn; the reply is the turn's end            |
//! | `session/cancel`  | interrupt the active turn                            |
//! | `session/list`    | snapshot live sessions                               |
//! | `session/close`   | interrupt + deregister                               |
//! | `session/delete`  | deregister                                           |
//! | `session/set_config_option` | choose the model a session runs on          |
//! | `_srud/unstable/session/set_title` | rename a session               |
//! | `_srud/unstable/provider/list` | list providers, with keys masked      |
//! | `_srud/unstable/provider/set` | add or replace a provider             |
//! | `_srud/unstable/provider/delete` | remove a provider                   |
//! | `_srud/unstable/provider/set_default` | choose new sessions' model     |
//!
//! Everything else — `session/set_mode`, `authenticate`, and the remaining
//! `_srud/unstable/*` extensions — answers `METHOD_NOT_FOUND`, matching what the
//! advertised capabilities promise.
//!
//! # Where a model client comes from
//!
//! A session names the model it runs on as a `providerId/modelId` reference, and
//! the agent resolves it per turn rather than holding one client for its whole
//! life. The reference travels to a client over ACP's own `configOptions`, so a
//! third-party client renders the same selector this one does; the credentials
//! behind it never leave this machine.
//!
//! An agent without a [`ProviderStore`] is one whose client was fixed at
//! construction: [`Agent::new`] takes one, which is what a test and an
//! environment-configured host both want. [`Agent::with_providers`] adds the
//! resolvable path on top.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use srud_core::client::ModelClient;
use srud_core::session::Session;
use srud_core::tools::ToolRegistry;
use srud_core::EventSink;
use srud_protocol::acp::methods::{
    INITIALIZE, SESSION_CANCEL, SESSION_CLOSE, SESSION_DELETE, SESSION_LIST, SESSION_LOAD,
    SESSION_NEW, SESSION_PROMPT, SESSION_RESUME, SESSION_SET_CONFIG_OPTION, SRUD_PROVIDER_DELETE,
    SRUD_PROVIDER_LIST, SRUD_PROVIDER_SET, SRUD_PROVIDER_SET_DEFAULT, SRUD_SESSION_SET_TITLE,
    SRUD_SESSION_STEER,
};
use srud_protocol::acp::{
    AcpError, CancelNotification, CloseSessionRequest, CloseSessionResponse, ConfigOptionUpdate,
    ContentBlock, DeleteSessionRequest, DeleteSessionResponse, Implementation, InitializeRequest,
    InitializeResponse, JsonRpcMessage, ListSessionsResponse, LoadSessionRequest,
    LoadSessionResponse, MaybeUndefined, NewSessionRequest, NewSessionResponse, Notification,
    PromptRequest, PromptResponse, ProtocolVersion, Request, RequestId, Response,
    ResumeSessionRequest, ResumeSessionResponse, SessionConfigOption, SessionId, SessionInfoUpdate,
    SessionNotification, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, CLIENT_METHOD_NAMES,
};
use srud_protocol::error::{
    INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, SESSION_BUSY,
    SESSION_NOT_FOUND,
};
use srud_protocol::srud::meta::{SrudMeta, SrudTurnEnd};
use srud_protocol::srud::methods::{
    DeleteProviderRequest, DeleteProviderResponse, ListProvidersRequest, ListProvidersResponse,
    ProviderCatalog, SetDefaultProviderRequest, SetDefaultProviderResponse, SetProviderRequest,
    SetProviderResponse, SetSessionTitleRequest, SetSessionTitleResponse, SteerRequest,
    SteerResponse,
};
use srud_protocol::srud::turn_end::TurnEndWire;

use crate::convert::{end_reason_wire, prompt_outcome, replay_notification};
use crate::events::{EventHub, TurnSink};
use crate::providers::{self, ModelFactory, ProviderError, ProviderStore};
use crate::sessions::{RestoreError, SessionManager};

/// The JSON-RPC reply to one `rpc_request` envelope.
pub type RpcReply = JsonRpcMessage<Response<Value>>;

/// The agent-side success reply the host resolves its promise with.
#[must_use]
pub fn reply_ok(id: RequestId, result: Value) -> RpcReply {
    JsonRpcMessage::wrap(Response::new(id, Ok(result)))
}

/// The agent-side error reply.
#[must_use]
pub fn reply_err(id: RequestId, error: AcpError) -> RpcReply {
    JsonRpcMessage::wrap(Response::new(id, Err(error)))
}

/// The provider configuration an agent resolves a session's model against.
///
/// The two halves belong together: the store says which providers exist and what
/// they offer, and the factory builds a client for one of them. Neither is much
/// use alone, and an agent has both or neither.
pub struct ModelProviders {
    /// The configured providers, and the credentials behind them.
    pub store: Arc<ProviderStore>,
    /// Builds a model client for a provider the store names.
    pub factory: Arc<dyn ModelFactory>,
}

impl ModelProviders {
    /// Pairs a provider store with the factory that builds its clients.
    #[must_use]
    pub fn new(store: Arc<ProviderStore>, factory: Arc<dyn ModelFactory>) -> Self {
        Self { store, factory }
    }
}

/// An ACP v1 agent backed by the core runtime.
pub struct Agent {
    /// The client used when a session names no model: one older than the
    /// provider file, which ran on whatever the host had fixed. `None` when the
    /// host fixed nothing, which is only possible alongside providers — there,
    /// every session that can run names its own model. Also the whole answer for
    /// an agent built without providers.
    model: Option<Arc<dyn ModelClient>>,
    /// The configurable providers, when the host supplied them.
    providers: Option<ModelProviders>,
    tools: Arc<ToolRegistry>,
    sessions: Arc<SessionManager>,
    hub: EventHub,
    active_sinks: Mutex<HashMap<SessionId, TurnSink>>,
    initialized: AtomicBool,
    /// Set once the host has begun shutting down. New prompts and steers are
    /// refused from that point on, so a drain cannot be extended by new work.
    shutting_down: AtomicBool,
    /// Held for reading by every prompt; [`Agent::shutdown`] probes the write
    /// side to see whether any is left.
    ///
    /// The write side can be taken only once every in-flight prompt has left,
    /// so a successful probe means every turn has settled.
    prompt_gate: tokio::sync::RwLock<()>,
}

impl Agent {
    /// Creates an agent over a model client and a tool registry.
    ///
    /// Every session runs on the given client: there is nothing to choose
    /// between, so `session/new` advertises no model selector and
    /// `session/set_config_option` answers `METHOD_NOT_FOUND`.
    #[must_use]
    pub fn new(model: Arc<dyn ModelClient>, tools: Arc<ToolRegistry>) -> Self {
        Self {
            model: Some(model),
            providers: None,
            tools,
            sessions: Arc::new(SessionManager::new()),
            hub: EventHub::new(),
            active_sinks: Mutex::new(HashMap::new()),
            initialized: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            prompt_gate: tokio::sync::RwLock::new(()),
        }
    }

    /// Creates an agent that resolves each session's model from a provider file.
    ///
    /// `model` is the fallback for a session that names no model — one created
    /// before any provider was configured, or restored from a log written then.
    /// `None` says the host has nothing to fall back to, which a host with
    /// providers genuinely can: a session that has no model to run on is told to
    /// choose one, rather than run on a model nobody picked.
    #[must_use]
    pub fn with_providers(
        model: Option<Arc<dyn ModelClient>>,
        tools: Arc<ToolRegistry>,
        providers: ModelProviders,
    ) -> Self {
        Self {
            model,
            providers: Some(providers),
            tools,
            sessions: Arc::new(SessionManager::new()),
            hub: EventHub::new(),
            active_sinks: Mutex::new(HashMap::new()),
            initialized: AtomicBool::new(false),
            shutting_down: AtomicBool::new(false),
            prompt_gate: tokio::sync::RwLock::new(()),
        }
    }

    /// Subscribes to the outgoing `session/update` notifications.
    #[must_use]
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<Notification<SessionNotification>> {
        self.hub.subscribe()
    }

    /// Stops accepting new prompts and steers, and waits for every in-flight
    /// turn to settle.
    ///
    /// Active turns are interrupted and re-interrupted while the drain waits,
    /// because a prompt that had already passed its entry check may claim its
    /// turn after the first signal.
    ///
    /// Returns whether every in-flight prompt finished before `timeout`.
    /// A turn that did not settle is left open in the log, exactly as after a
    /// process crash; the next load or resume closes it with a synthetic
    /// interruption.
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        self.shutting_down.store(true, Ordering::Release);

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            self.sessions.interrupt_all();

            // Every prompt holds a read guard for its whole turn, so a write
            // that becomes available means none is left. Probing rather than
            // awaiting the lock keeps the loop free to re-signal a turn that
            // claims the session late.
            if self.prompt_gate.try_write().is_ok() {
                return true;
            }

            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Handles one JSON-RPC request and produces its reply.
    ///
    /// `session/prompt` resolves only when the turn ends — that is the
    /// protocol's completion signal. All other methods reply immediately.
    pub async fn handle(&self, request: JsonRpcMessage<Request<Value>>) -> RpcReply {
        let request = request.into_inner();
        let id = request.id;
        let params = request.params.unwrap_or(Value::Null);
        let method = request.method.as_ref();

        // `initialize` is the mandatory first request; everything else is
        // refused until the negotiation has happened.
        if method != INITIALIZE && !self.initialized.load(Ordering::Acquire) {
            return reply_err(
                id,
                AcpError::new(
                    INVALID_REQUEST,
                    "initialize must be the first request on the connection",
                ),
            );
        }

        let outcome = match method {
            INITIALIZE => self.handle_initialize(params).map(serialize),
            SESSION_NEW => self.handle_new_session(params).await.map(serialize),
            SESSION_LOAD => self.handle_load(params).await.map(serialize),
            SESSION_RESUME => self.handle_resume(params).await.map(serialize),
            SESSION_PROMPT => self.handle_prompt(params).await.map(serialize),
            SESSION_CANCEL => self
                .handle_cancel(params)
                .map(|()| Value::Object(Default::default())),
            SESSION_LIST => self.handle_list().await.map(serialize),
            SESSION_CLOSE => self
                .handle_close(params)
                .await
                .map(|()| serialize(CloseSessionResponse::new())),
            SESSION_DELETE => self
                .handle_delete(params)
                .await
                .map(|()| serialize(DeleteSessionResponse::new())),
            SESSION_SET_CONFIG_OPTION => self.handle_set_config_option(params).await.map(serialize),
            SRUD_SESSION_SET_TITLE => self.handle_set_title(params).await.map(serialize),
            SRUD_SESSION_STEER => self.handle_steer(params).await.map(serialize),
            SRUD_PROVIDER_LIST => self.handle_provider_list(params),
            SRUD_PROVIDER_SET => self.handle_provider_set(params),
            SRUD_PROVIDER_DELETE => self.handle_provider_delete(params),
            SRUD_PROVIDER_SET_DEFAULT => self.handle_provider_set_default(params),
            // Known-but-unimplemented methods get the same treatment as
            // unknown ones: the capabilities never advertised them.
            _ => Err(AcpError::new(
                METHOD_NOT_FOUND,
                format!("method `{method}` is not supported"),
            )),
        };

        match outcome {
            Ok(result) => reply_ok(id, result),
            Err(error) => reply_err(id, error),
        }
    }

    fn handle_initialize(&self, params: Value) -> Result<InitializeResponse, AcpError> {
        let request: InitializeRequest = parse_params(params)?;
        if request.protocol_version != ProtocolVersion::V1 {
            return Err(AcpError::new(
                INVALID_PARAMS,
                format!(
                    "unsupported protocol version {}; this agent speaks v1 only",
                    request.protocol_version
                ),
            ));
        }
        self.initialized.store(true, Ordering::Release);
        Ok(InitializeResponse::new(ProtocolVersion::V1)
            .agent_capabilities(srud_protocol::acp::capabilities::agent_capabilities())
            .agent_info(Implementation::new("srud-agent", env!("CARGO_PKG_VERSION")))
            .auth_methods(vec![]))
    }

    async fn handle_new_session(&self, params: Value) -> Result<NewSessionResponse, AcpError> {
        let request: NewSessionRequest = parse_params(params)?;
        // Seeded from the configured default so the session and its log agree
        // from the first record, and nothing later has to guess what it was
        // running on before anyone chose. Nothing is seeded when no default is
        // set — the session opens with the picker waiting rather than with a
        // model its user never named.
        let start = self
            .providers
            .as_ref()
            .and_then(|providers| providers.store.start_reference());
        let session_id = self
            .sessions
            .create(Some(request.cwd), start)
            .await
            .map_err(|err| {
                AcpError::new(
                    INTERNAL_ERROR,
                    format!("could not create the session workspace: {err}"),
                )
            })?;
        let mut response = NewSessionResponse::new(session_id.clone());
        // Offered to a client that may render it, and ignored by one that does
        // not know the category — which is the whole reason it travels here
        // rather than through a method of SrudAgent's own.
        if let Some(options) = self.config_options(&session_id) {
            response = response.config_options(options);
        }
        Ok(response)
    }

    async fn handle_prompt(&self, params: Value) -> Result<PromptResponse, AcpError> {
        // Held for the whole turn, so `shutdown`'s probe succeeds only once the
        // turn has settled.
        let _running = self.prompt_gate.read().await;
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AcpError::new(INTERNAL_ERROR, "the agent is shutting down"));
        }

        let request: PromptRequest = parse_params(params)?;

        // The session and its log together, so a turn cannot be recorded against
        // another session's log. One lookup rather than two: the pair is
        // registered together and is never separated.
        let (session, log) = self
            .sessions
            .with_log(&request.session_id)
            .ok_or_else(|| session_not_found(&request.session_id))?;

        // Resolved before anything is named or recorded: a prompt with no model
        // to run on is not a turn, and a session named by one would be named
        // after a message that never ran.
        let client = self.client_for(&request.session_id)?;

        let parts = prompt_parts(&request.prompt)?;

        let sink = self
            .hub
            .sink_for(session.id(), self.context_size_for(&request.session_id));
        // Registering the turn's sink is the busy gate: steer announcements reach
        // the running turn through this map, so a second prompt must neither
        // overwrite the live sink nor remove it when it unwinds. The gate also
        // precedes naming, so a refused prompt leaves no trace.
        {
            let mut sinks = self
                .active_sinks
                .lock()
                .expect("active sinks lock poisoned");
            if sinks.contains_key(&request.session_id) {
                return Err(AcpError::new(
                    SESSION_BUSY,
                    "session already has an active turn",
                ));
            }
            sinks.insert(request.session_id.clone(), sink.clone());
        }

        self.name_from_first_prompt(&request.session_id, &parts.text)
            .await;

        let result = srud_core::turn::run_turn(
            &session,
            srud_core::types::TurnInput {
                text: parts.text,
                images: parts.images,
            },
            client.as_ref(),
            self.tools.as_ref(),
            log.as_ref(),
            &sink,
        )
        .await;
        self.active_sinks
            .lock()
            .expect("active sinks lock poisoned")
            .remove(&request.session_id);
        let result = result.map_err(|err| match err {
            srud_core::TurnError::Busy => {
                AcpError::new(SESSION_BUSY, "session already has an active turn")
            }
            // Distinct from `Busy`: nothing ran, and the session is still usable —
            // the log is what refused. Saying "busy" would tell the client to wait
            // for something that will never start.
            srud_core::TurnError::Unrecordable(err) => {
                AcpError::new(INTERNAL_ERROR, format!("cannot record this turn: {err}"))
            }
        })?;

        let wire: TurnEndWire = prompt_outcome(result.reason)?;
        let response = PromptResponse::new(wire.stop_reason);
        Ok(match wire.meta {
            Some(meta) => response.meta(meta.to_meta()),
            None => response,
        })
    }

    fn handle_cancel(&self, params: Value) -> Result<(), AcpError> {
        let request: CancelNotification = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        // Cooperative: this signals the turn's token; the prompt reply then
        // carries `stopReason: "cancelled"`.
        session.interrupt();
        Ok(())
    }

    async fn handle_list(&self) -> Result<ListSessionsResponse, AcpError> {
        Ok(ListSessionsResponse::new(self.sessions.list().await))
    }

    async fn handle_steer(&self, params: Value) -> Result<SteerResponse, AcpError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(AcpError::new(INTERNAL_ERROR, "the agent is shutting down"));
        }

        let request: SteerRequest = parse_params(params)?;
        let session = self
            .sessions
            .get(&request.session_id)
            .ok_or_else(|| session_not_found(&request.session_id))?;
        let parts = prompt_parts(&request.prompt)?;
        if !parts.images.is_empty() {
            return Err(AcpError::new(
                INVALID_PARAMS,
                "steering accepts text prompts only",
            ));
        }
        if !session.steer(&parts.text) {
            return Err(AcpError::new(
                SESSION_BUSY,
                "session has no active turn to steer",
            ));
        }
        // Announce the injected message the way a normal prompt does.
        if let Some(sink) = self
            .active_sinks
            .lock()
            .expect("active sinks lock poisoned")
            .get(&request.session_id)
            .cloned()
        {
            sink.emit(srud_core::types::Event::UserMessage {
                content: parts.text.clone(),
            });
        }
        Ok(SteerResponse::default())
    }

    async fn handle_close(&self, params: Value) -> Result<(), AcpError> {
        let request: CloseSessionRequest = parse_params(params)?;
        let session = self.require_session(&request.session_id)?;
        session.interrupt();
        // Closing leaves the log where it is: the session is over, not deleted, and
        // is what the next `session/list` reports so a client can load it again.
        self.sessions.forget(&request.session_id);
        Ok(())
    }

    async fn handle_delete(&self, params: Value) -> Result<(), AcpError> {
        let request: DeleteSessionRequest = parse_params(params)?;
        if !self.sessions.remove(&request.session_id).await {
            return Err(session_not_found(&request.session_id));
        }
        Ok(())
    }

    async fn handle_resume(&self, params: Value) -> Result<ResumeSessionResponse, AcpError> {
        let request: ResumeSessionRequest = parse_params(params)?;
        let loaded = self.restore(&request.session_id, &request.cwd).await?;
        // No replay: a client asking to resume is saying it already has the
        // conversation.
        self.broadcast_title(&request.session_id, loaded.title.clone());
        let mut response = ResumeSessionResponse::new();
        if let Some(options) = self.config_options(&request.session_id) {
            response = response.config_options(options);
        }
        Ok(response)
    }

    async fn handle_load(&self, params: Value) -> Result<LoadSessionResponse, AcpError> {
        let request: LoadSessionRequest = parse_params(params)?;
        let loaded = self.restore(&request.session_id, &request.cwd).await?;

        // Replayed before the response, because ACP requires the client to have
        // the whole conversation by the time `session/load` resolves. Anything
        // sent after would arrive as a change to a session the client already
        // believes it has.
        let context_size = self.context_size_for(&request.session_id);
        for event in &loaded.conversation {
            if let Some(notification) =
                replay_notification(&request.session_id, event, context_size)
            {
                self.hub.send(notification);
            }
        }
        self.broadcast_title(&request.session_id, loaded.title.clone());
        let mut response = LoadSessionResponse::new();
        // ACP has this response carry the options, so a client being shown a
        // session is told which model it ran on without asking.
        if let Some(options) = self.config_options(&request.session_id) {
            response = response.config_options(options);
        }
        // How each turn ended travels here: a replayed item is the update live
        // streaming sent, and none of those carries a turn boundary. A turn the log
        // left open is absent from the list, which a client reads as still running.
        let turns: Vec<SrudTurnEnd> = loaded
            .turn_ends
            .iter()
            .map(|(turn_id, reason)| SrudTurnEnd {
                turn_id: turn_id.to_string(),
                end_reason: end_reason_wire(*reason).to_string(),
            })
            .collect();
        let srud = SrudMeta {
            turns: Some(turns),
            ..Default::default()
        };
        Ok(response.meta(srud.to_meta()))
    }

    /// Brings a session back, or says why it could not be.
    async fn restore(
        &self,
        id: &SessionId,
        cwd: &std::path::Path,
    ) -> Result<srud_core::session_store::Loaded, AcpError> {
        self.sessions.restore(id, cwd).await.map_err(restore_failed)
    }

    /// Names an unnamed session after the message that opened it.
    ///
    /// A session is named by its first prompt and never re-derived: later turns
    /// must not overwrite it, and a rename the user has already made wins. When
    /// this call is the one that named the session, it broadcasts the title so
    /// every attached client renders it without asking.
    ///
    /// A title that cannot be recorded is not announced, but does not fail the
    /// prompt: the name is cosmetic, and the turn it opened still has to run. The
    /// failure is dropped, which is the one place that is the right answer — a
    /// disk that refused the title will refuse the turn's own records a moment
    /// later and report it there.
    async fn name_from_first_prompt(&self, session_id: &SessionId, text: &str) {
        let title = title_from_prompt(text);
        if let Ok(true) = self.sessions.name_if_unnamed(session_id, &title).await {
            self.broadcast_title(session_id, Some(title));
        }
    }

    async fn handle_set_title(&self, params: Value) -> Result<SetSessionTitleResponse, AcpError> {
        let request: SetSessionTitleRequest = parse_params(params)?;
        match self
            .sessions
            .set_title(&request.session_id, &request.title)
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(session_not_found(&request.session_id)),
            Err(err) => {
                return Err(AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot record the title: {err}"),
                ));
            }
        }
        // Echo the effective title rather than the request's, so a blank title
        // is reported as the clear it was.
        let title = self.sessions.title(&request.session_id);
        self.broadcast_title(&request.session_id, title.clone());
        Ok(SetSessionTitleResponse { title, meta: None })
    }

    /// The model client a session runs on.
    ///
    /// Resolved per turn rather than held for the agent's life: the selection can
    /// change between two turns of one session, and a turn has to run on
    /// whichever model was chosen when it started.
    fn client_for(&self, id: &SessionId) -> Result<Arc<dyn ModelClient>, AcpError> {
        let Some(providers) = &self.providers else {
            return self.fixed_client();
        };
        // A session that names no model is one created before any provider was
        // configured, or restored from a log written then. It runs on the client
        // the host supplied, which is what it ran on before — and when the host
        // supplied none, nothing is picked on the user's behalf. This is the
        // path a session takes when nobody has chosen its model yet, or when the
        // configured default names something that is no longer there. Saying so
        // is more use than running on a model nobody asked for.
        let Some(reference) = self.sessions.model(id) else {
            return self.fixed_client().map_err(|_| {
                AcpError::new(
                    INVALID_PARAMS,
                    "this session has no model yet; choose one from the model picker",
                )
            });
        };

        let (provider, model) = providers.store.resolve(&reference).map_err(|err| {
            // The session names a model that is no longer configured. Refused
            // rather than quietly run on something else: a turn recorded under
            // one model while another answered would make the log a lie about
            // what produced it.
            AcpError::new(
                INVALID_PARAMS,
                format!("this session's model is no longer configured: {err}"),
            )
        })?;
        providers
            .factory
            .build(&provider, &model.id)
            .map_err(|err| {
                AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot build a client for `{reference}`: {err}"),
                )
            })
    }

    /// The configured context window of the session's model.
    ///
    /// `None` when the host configured no providers, the session names no
    /// model, the model is no longer configured, or no window was set for it —
    /// in each case usage reports simply go unshown rather than guessed at.
    fn context_size_for(&self, id: &SessionId) -> Option<u64> {
        let providers = self.providers.as_ref()?;
        let reference = self.sessions.model(id)?;
        let (_provider, model) = providers.store.resolve(&reference).ok()?;
        model.context_window
    }

    /// The client the host fixed at construction, when it fixed one.
    fn fixed_client(&self) -> Result<Arc<dyn ModelClient>, AcpError> {
        self.model.clone().ok_or_else(|| {
            AcpError::new(
                INTERNAL_ERROR,
                "the host supplied no model client and this session names no model",
            )
        })
    }

    /// The model selector, as this session should be offered it.
    ///
    /// `None` when the host configured no providers: an agent with one fixed
    /// client has nothing to choose between, and offering an empty selector
    /// would claim it had.
    fn config_options(&self, id: &SessionId) -> Option<Vec<SessionConfigOption>> {
        let providers = self.providers.as_ref()?;
        providers
            .store
            .session_config_options(self.sessions.model(id).as_deref())
    }

    /// Chooses the model a session runs on.
    ///
    /// ACP's own method, not an extension: a client that knows nothing about
    /// SrudAgent still renders the selector and can still switch. That is also
    /// why the *global* default is not changed here — this request has a session
    /// id, and there is no way to say "and every other session too" in it.
    async fn handle_set_config_option(
        &self,
        params: Value,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        let request: SetSessionConfigOptionRequest = parse_params(params)?;
        let Some(providers) = &self.providers else {
            return Err(AcpError::new(
                METHOD_NOT_FOUND,
                "this agent's model is not configurable",
            ));
        };
        if request.config_id.0.as_ref() != providers::MODEL_OPTION_ID {
            return Err(AcpError::new(
                INVALID_PARAMS,
                format!("no configuration option `{}`", request.config_id.0),
            ));
        }
        let reference = request
            .value
            .as_value_id()
            .ok_or_else(|| AcpError::new(INVALID_PARAMS, "the model option takes a string value"))?
            .0
            .to_string();

        // Resolved before it is recorded. A reference nothing resolves to would
        // leave the session unable to take a turn, while its log claimed the
        // choice was deliberate.
        providers
            .store
            .resolve(&reference)
            .map_err(|err| AcpError::new(INVALID_PARAMS, err.to_string()))?;

        match self
            .sessions
            .set_model(&request.session_id, Some(reference))
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(session_not_found(&request.session_id)),
            Err(err) => {
                return Err(AcpError::new(
                    INTERNAL_ERROR,
                    format!("cannot record the model: {err}"),
                ));
            }
        }

        // The response carries the whole set rather than an acknowledgement, so
        // a client renders the result of its own change without asking again.
        match self.config_options(&request.session_id) {
            Some(options) => {
                self.broadcast_config_options(&request.session_id, options.clone());
                Ok(SetSessionConfigOptionResponse::new(options))
            }
            None => Ok(SetSessionConfigOptionResponse::new(Vec::new())),
        }
    }

    /// The provider configuration, when the host supplied one.
    fn provider_store(&self) -> Result<&Arc<ProviderStore>, AcpError> {
        self.providers
            .as_ref()
            .map(|providers| &providers.store)
            .ok_or_else(|| {
                AcpError::new(
                    METHOD_NOT_FOUND,
                    "this agent was not given a provider configuration",
                )
            })
    }

    fn handle_provider_list(&self, params: Value) -> Result<Value, AcpError> {
        let _: ListProvidersRequest = parse_params(params)?;
        let store = self.provider_store()?;
        Ok(serialize(ListProvidersResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    fn handle_provider_set(&self, params: Value) -> Result<Value, AcpError> {
        let request: SetProviderRequest = parse_params(params)?;
        let store = self.provider_store()?;
        store
            .upsert(providers::ProviderInput {
                id: request.provider.id,
                name: request.provider.name,
                base_url: request.provider.base_url,
                api_key: request.provider.api_key,
                models: request
                    .provider
                    .models
                    .into_iter()
                    .map(|model| providers::Model {
                        id: model.id,
                        name: model.name,
                        modalities: model.modalities,
                        context_window: model.context_window,
                    })
                    .collect(),
            })
            .map_err(provider_rejected)?;
        Ok(serialize(SetProviderResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    fn handle_provider_delete(&self, params: Value) -> Result<Value, AcpError> {
        let request: DeleteProviderRequest = parse_params(params)?;
        let store = self.provider_store()?;
        store
            .delete(&request.provider_id)
            .map_err(provider_rejected)?;
        Ok(serialize(DeleteProviderResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    fn handle_provider_set_default(&self, params: Value) -> Result<Value, AcpError> {
        let request: SetDefaultProviderRequest = parse_params(params)?;
        let store = self.provider_store()?;
        store
            .set_default(&request.model)
            .map_err(provider_rejected)?;
        Ok(serialize(SetDefaultProviderResponse {
            catalog: catalog(store),
            meta: None,
        }))
    }

    /// Publishes a configuration change on the `session/update` stream.
    ///
    /// The same channel `session_info_update` travels on, so a client already
    /// following `session/update` sees a model switched by another client
    /// without having asked.
    fn broadcast_config_options(&self, session_id: &SessionId, options: Vec<SessionConfigOption>) {
        self.hub.send(Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(options)),
            )),
        });
    }

    /// Publishes a title change on the `session/update` stream.
    ///
    /// ACP routes this through the standard progress channel as a
    /// `SessionInfoUpdate` variant rather than a method of its own, so a client
    /// already following `session/update` sees renames for free.
    fn broadcast_title(&self, session_id: &SessionId, title: Option<String>) {
        // The builder takes `IntoMaybeUndefined`, so a `String` becomes a set
        // title and `MaybeUndefined::Null` clears it — how ACP spells "no title".
        let title: MaybeUndefined<String> = match title {
            Some(title) => MaybeUndefined::Value(title),
            None => MaybeUndefined::Null,
        };
        let update = SessionInfoUpdate::new().title(title);
        self.hub.send(Notification {
            method: CLIENT_METHOD_NAMES.session_update.into(),
            params: Some(SessionNotification::new(
                session_id.clone(),
                SessionUpdate::SessionInfoUpdate(update),
            )),
        });
    }

    fn require_session(&self, id: &SessionId) -> Result<Arc<Session>, AcpError> {
        self.sessions.get(id).ok_or_else(|| session_not_found(id))
    }
}

/// The provider catalog as the wire reports it.
fn catalog(store: &ProviderStore) -> ProviderCatalog {
    ProviderCatalog {
        providers: store.views(),
        default_model: store.default_reference(),
    }
}

/// What a client is told when a provider change was refused.
///
/// A fault in the request is told apart from a fault in the machine: an id or a
/// URL the caller got wrong is theirs to fix, while a file that cannot be written
/// is not something they can do anything about.
fn provider_rejected(err: ProviderError) -> AcpError {
    let code = match &err {
        ProviderError::Invalid(_)
        | ProviderError::BadReference(_)
        | ProviderError::UnknownProvider(_)
        | ProviderError::UnknownModel(_) => INVALID_PARAMS,
        ProviderError::Config(_) | ProviderError::Io { .. } | ProviderError::Parse { .. } => {
            INTERNAL_ERROR
        }
    };
    AcpError::new(code, err.to_string())
}

/// Derives a session title from the message that opened it.
///
/// The message is a whole paragraph, so it is flattened to one line and cut to
/// a length a sidebar row can show. Truncation is on a character boundary, not
/// a byte one, so multi-byte text cannot panic.
fn title_from_prompt(text: &str) -> String {
    const MAX_CHARS: usize = 60;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_CHARS).collect();
    // Drop a trailing partial word so the title does not end mid-word.
    match cut.rfind(' ') {
        Some(idx) if idx > MAX_CHARS / 2 => cut[..idx].to_string(),
        _ => cut,
    }
}

fn session_not_found(id: &SessionId) -> AcpError {
    AcpError::new(
        SESSION_NOT_FOUND,
        format!("no session with id `{}`", id.0.as_ref()),
    )
}

/// What a client is told when a session could not be brought back.
fn restore_failed(err: RestoreError) -> AcpError {
    match &err {
        RestoreError::NotFound(id) => session_not_found(id),
        // The caller named a different directory, which ACP requires to match:
        // that is a bad request, not a server fault.
        RestoreError::Cwd { .. } => AcpError::new(INVALID_PARAMS, err.to_string()),
        RestoreError::Config(_) | RestoreError::Load(_) | RestoreError::Repair(_) => {
            AcpError::new(INTERNAL_ERROR, format!("cannot load the session: {err}"))
        }
    }
}

/// Deserialises method params, reporting invalid payloads as
/// `INVALID_PARAMS` with the serde message attached.
fn parse_params<T>(params: Value) -> Result<T, AcpError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(params)
        .map_err(|err| AcpError::new(INVALID_PARAMS, format!("invalid method params: {err}")))
}

fn serialize<T: serde::Serialize>(value: T) -> Value {
    serde_json::to_value(value)
        .unwrap_or_else(|err| Value::String(format!("response serialization failed: {err}")))
}

/// A prompt block that the declared capabilities do not accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedContent {
    /// An audio block, refused while `promptCapabilities.audio` is false.
    Audio,
    /// Any other block the agent does not read, such as a resource or a link.
    Other(&'static str),
}

/// The parts of a prompt the agent accepts.
#[derive(Debug, Default)]
pub struct PromptParts {
    /// The text blocks, joined in the order they arrived.
    pub text: String,
    /// The image blocks, in the order they arrived.
    pub images: Vec<srud_core::types::ImagePart>,
}

/// Splits a prompt into the text and images the agent accepts.
///
/// Returns an error naming the first block the agent cannot accept — dropping
/// multimodal input silently would make the model answer a prompt the user
/// never sent.
pub fn prompt_parts(blocks: &[ContentBlock]) -> Result<PromptParts, AcpError> {
    let mut parts = PromptParts::default();
    for block in blocks {
        match block {
            ContentBlock::Text(content) => {
                if !parts.text.is_empty() {
                    parts.text.push('\n');
                }
                parts.text.push_str(&content.text);
            }
            ContentBlock::Image(content) => parts.images.push(srud_core::types::ImagePart {
                mime: content.mime_type.clone(),
                data: content.data.clone(),
            }),
            block => {
                let kind = match block {
                    ContentBlock::Audio(_) => UnsupportedContent::Audio,
                    ContentBlock::Resource(_) => UnsupportedContent::Other("resource"),
                    ContentBlock::ResourceLink(_) => UnsupportedContent::Other("resource_link"),
                    _ => UnsupportedContent::Other("unknown"),
                };
                return Err(AcpError::new(
                    INVALID_PARAMS,
                    format!("this agent cannot accept {kind:?} prompts"),
                ));
            }
        }
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use crate::providers::{Model, Provider, ProviderInput};
    use crate::test_env::{self, Guard};
    use futures::{stream, StreamExt};
    use serde_json::json;
    use srud_core::client::{ModelEvent, ModelRequest, ModelStream, TokenUsage};
    use srud_protocol::acp::methods::{
        SESSION_LIST, SRUD_SESSION_FORK, SRUD_SESSION_ROLLOUT_READ, SRUD_SESSION_SET_TITLE,
    };
    use srud_protocol::acp::TextContent;
    use srud_protocol::acp::AGENT_METHOD_NAMES;
    use srud_protocol::transport::tauri::RpcRequest;
    use tokio_util::sync::CancellationToken;

    /// The method names that answer `METHOD_NOT_FOUND`, so tests can assert
    /// the coverage boundary.
    const UNIMPLEMENTED_METHODS: &[&str] = &[
        AGENT_METHOD_NAMES.session_set_mode,
        AGENT_METHOD_NAMES.authenticate,
        SRUD_SESSION_FORK,
        SRUD_SESSION_ROLLOUT_READ,
    ];

    /// Prompts a session, drains its turn, and returns the broadcast titles.
    ///
    /// The subscriber is attached before the prompt so the title broadcast on
    /// the first turn cannot be missed.
    async fn prompt_titles(
        agent: &Agent,
        session_id: &SessionId,
        text: &str,
    ) -> Vec<Option<String>> {
        let mut rx = agent.subscribe();
        agent
            .handle(request(
                SESSION_PROMPT,
                json!({
                    "sessionId": session_id.0.as_ref(),
                    "prompt": [{ "type": "text", "text": text }]
                }),
            ))
            .await;
        let mut titles = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            if value["params"]["sessionId"] != serde_json::json!(session_id.0.as_ref()) {
                continue;
            }
            if value["params"]["update"]["sessionUpdate"]
                == serde_json::json!("session_info_update")
            {
                titles.push(
                    value["params"]["update"]["title"]
                        .as_str()
                        .map(ToString::to_string),
                );
            }
        }
        titles
    }

    struct Scripted {
        events: Vec<ModelEvent>,
        /// When set, the stream stalls until the turn's cancellation token
        /// fires, then yields one more delta so the loop notices.
        stall_until_cancel: bool,
    }

    #[async_trait::async_trait]
    impl ModelClient for Scripted {
        async fn stream(
            &self,
            _request: ModelRequest,
            cancel: CancellationToken,
        ) -> Result<ModelStream, srud_core::client::ModelError> {
            let head: ModelStream = Box::pin(stream::iter(self.events.clone().into_iter().map(Ok)));
            let tail: ModelStream = if self.stall_until_cancel {
                // Stall until the turn is cancelled, emit one more delta to
                // wake the loop, then end. The loop's next cancellation check
                // turns this into `Interrupted`.
                Box::pin(stream::unfold(Some(cancel), |state| async move {
                    let token = state?;
                    token.cancelled().await;
                    Some((
                        Ok(ModelEvent::TextDelta {
                            delta: "after-cancel".into(),
                        }),
                        None,
                    ))
                }))
            } else {
                Box::pin(stream::iter(std::iter::once(Ok(ModelEvent::Done))))
            };
            Ok(Box::pin(head.chain(tail)))
        }
    }

    fn scripted(events: Vec<ModelEvent>) -> Arc<dyn ModelClient> {
        Arc::new(Scripted {
            events,
            stall_until_cancel: false,
        })
    }

    fn stalling(events: Vec<ModelEvent>) -> Arc<dyn ModelClient> {
        Arc::new(Scripted {
            events,
            stall_until_cancel: true,
        })
    }

    fn agent(model: Arc<dyn ModelClient>) -> Agent {
        Agent::new(model, Arc::new(ToolRegistry::new()))
    }

    fn request(method: &str, params: Value) -> RpcRequest {
        JsonRpcMessage::wrap(Request {
            id: RequestId::Number(1),
            method: method.into(),
            params: Some(params),
        })
    }

    async fn initialize(agent: &Agent) {
        let reply = agent
            .handle(request(
                INITIALIZE,
                json!({ "protocolVersion": 1, "clientCapabilities": {} }),
            ))
            .await;
        assert!(
            matches!(reply.inner(), Response::Result { .. }),
            "initialize should succeed: {reply:?}"
        );
    }

    async fn new_session(agent: &Agent) -> SessionId {
        // The workspace is settled from the configuration directory, which is
        // read from the environment. Holding the environment and pointing it at
        // a directory of this test's own keeps the session off the developer's
        // home and off whatever another test left behind.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-agent"),
        );
        new_session_in(agent, "").await
    }

    /// Creates a session working in `cwd`, or with a workspace of its own when
    /// `cwd` is empty.
    ///
    /// Without the environment setup `new_session` does, for a test that has to
    /// keep one configuration directory alive across two agents.
    async fn new_session_in(agent: &Agent, cwd: &str) -> SessionId {
        let reply = agent
            .handle(request(
                SESSION_NEW,
                // No working directory named, so the session gets a workspace of
                // its own. Spelled out rather than left to a path that may or may
                // not exist on the machine running the test.
                json!({ "cwd": cwd, "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new should succeed");
        };
        SessionId::new(result["sessionId"].as_str().unwrap().to_string())
    }

    /// Every `session/update` sent for a session, as `(kind, text)`.
    ///
    /// `session_info_update` is left out: it carries the title rather than the
    /// conversation, and it travels on the same channel.
    fn conversation(
        rx: &mut tokio::sync::broadcast::Receiver<Notification<SessionNotification>>,
    ) -> Vec<(String, String)> {
        let mut updates = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            let update = &value["params"]["update"];
            let Some(kind) = update["sessionUpdate"].as_str() else {
                continue;
            };
            if kind == "session_info_update" {
                continue;
            }
            updates.push((
                kind.to_string(),
                update["content"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ));
        }
        updates
    }

    /// Every `session/update` sent for a session, as `(turn id, turn end)`.
    ///
    /// The title update is left out, as in [`conversation`]: it belongs to the
    /// session rather than to a turn. A replayed update carries no turn end — that
    /// is on the `session/load` response — so the second half is always `None`.
    fn replayed_updates(
        rx: &mut tokio::sync::broadcast::Receiver<Notification<SessionNotification>>,
    ) -> Vec<(String, Option<String>)> {
        let mut seen = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            if value["params"]["update"]["sessionUpdate"] == json!("session_info_update") {
                continue;
            }
            let srud = &value["params"]["_meta"]["srud"];
            seen.push((
                srud["turnId"].as_str().unwrap_or_default().to_string(),
                srud["turnEndReason"].as_str().map(str::to_string),
            ));
        }
        seen
    }

    fn result_of(reply: RpcReply) -> Value {
        match reply.into_inner() {
            Response::Result { result, .. } => result,
            Response::Error { error, .. } => panic!("expected success, got {error:?}"),
        }
    }

    fn error_of(reply: RpcReply) -> AcpError {
        match reply.into_inner() {
            Response::Error { error, .. } => error,
            Response::Result { result, .. } => panic!("expected error, got {result:?}"),
        }
    }

    #[tokio::test]
    async fn requests_before_initialize_are_refused() {
        let agent = agent(scripted(vec![]));
        let error = error_of(
            agent
                .handle(request(SESSION_NEW, json!({ "cwd": "/" })))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_REQUEST);
    }

    #[tokio::test]
    async fn initialize_negotiates_v1_and_capabilities() {
        let agent = agent(scripted(vec![]));
        let result = result_of(
            agent
                .handle(request(
                    INITIALIZE,
                    json!({ "protocolVersion": 1, "clientCapabilities": {} }),
                ))
                .await,
        );
        assert_eq!(result["protocolVersion"], json!(1));
        assert_eq!(result["agentCapabilities"]["loadSession"], json!(true));
        assert!(result["agentCapabilities"]["sessionCapabilities"]["list"].is_object());
        assert!(result["agentCapabilities"]["sessionCapabilities"]["resume"].is_object());
        assert!(result.get("authMethods").is_some());
    }

    /// A session and the directory it works in, on a configuration directory of
    /// this test's own.
    ///
    /// Returned together because a load has to name the directory ACP will check
    /// against, and the caller has to keep the guard alive for it to be found.
    async fn session_with_a_home(agent: &Agent) -> (SessionId, String) {
        let cwd = std::env::temp_dir().join(format!("srud-load-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&cwd).expect("a directory to work in");
        let cwd = cwd.to_string_lossy().into_owned();
        let id = new_session_in(agent, &cwd).await;
        (id, cwd)
    }

    #[tokio::test]
    async fn a_session_loads_back_from_its_log() {
        // The point of the log: an agent that restarted has nothing in memory and
        // everything on disk.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        // A second agent, as after a restart.
        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx),
            vec![
                ("user_message_chunk".to_string(), "hello".to_string()),
                ("agent_message_chunk".to_string(), "hi there".to_string()),
            ],
            "the conversation is replayed in order"
        );
    }

    #[tokio::test]
    async fn a_loaded_session_continues_where_it_left_off() {
        // Loading is not just replay: the agent has to have the history too, or
        // the next prompt answers as though the session were new.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-continue"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "first answer".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "first question").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        let session = second
            .sessions
            .get(&session_id)
            .expect("the session is live");
        assert_eq!(
            session.state().history(),
            &[
                // The env-context block the first turn recorded: it is part of
                // what the model saw, so it comes back with it.
                srud_core::context::item(std::path::Path::new(&cwd)),
                srud_core::types::ResponseItem::user("first question"),
                srud_core::types::ResponseItem::assistant("first answer"),
            ]
        );
    }

    #[tokio::test]
    async fn resuming_does_not_replay_the_conversation() {
        // What a client asks for by resuming: it already has the transcript.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-resume"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_RESUME,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert!(conversation(&mut rx).is_empty(), "nothing is replayed");
        assert!(second.sessions.get(&session_id).is_some(), "and it is live");
    }

    #[tokio::test]
    async fn a_live_session_loads_for_another_client() {
        // One client has the session open and another asks for it: the
        // conversation comes off the log, and nothing is invented for a session
        // that never stopped.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-live"),
        );

        let agent = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&agent).await;
        let (session_id, cwd) = session_with_a_home(&agent).await;
        prompt_titles(&agent, &session_id, "hello").await;

        let mut rx = agent.subscribe();
        result_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx),
            vec![
                ("user_message_chunk".to_string(), "hello".to_string()),
                ("agent_message_chunk".to_string(), "hi there".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn loading_a_session_that_never_existed_is_not_found() {
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-missing"),
        );

        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": "00000000-0000-4000-8000-000000000000",
                        "cwd": std::env::temp_dir().to_string_lossy(),
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn loading_in_another_directory_is_refused() {
        // ACP requires the caller's cwd to match the session's, and it has to:
        // the history was produced somewhere.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-cwd-mismatch"),
        );

        let first = agent(scripted(vec![]));
        initialize(&first).await;
        let (session_id, _cwd) = session_with_a_home(&first).await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let error = error_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": std::env::temp_dir().to_string_lossy(),
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn loading_without_naming_a_directory_is_allowed() {
        // A client that named no directory is not disagreeing with the recorded
        // one, so it is not refused here: the app has no directory picker and says
        // so on load the same way it does when it creates a session. Refusing would
        // leave every session from a previous run unloadable, which after a restart
        // is all of them.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-cwd-absent"),
        );

        let first = agent(scripted(vec![ModelEvent::TextDelta {
            delta: "hi there".into(),
        }]));
        initialize(&first).await;
        let (session_id, _cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": "",
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        assert_eq!(
            conversation(&mut rx).len(),
            2,
            "the conversation comes back, not an empty session"
        );
    }

    #[tokio::test]
    async fn the_load_response_says_how_each_turn_ended() {
        // A client being shown a session has no `session/prompt` response to close
        // its turns with, so the response to `session/load` lists them.
        let _env = crate::test_env::Guard::take(&[crate::config::HOME_VAR]);
        std::env::set_var(
            crate::config::HOME_VAR,
            crate::test_env::unique_dir("srud-load-turn-ends"),
        );

        // Each turn reports usage, so each one's log ends with a usage record.
        let first = agent(scripted(vec![
            ModelEvent::TextDelta {
                delta: "answer".into(),
            },
            ModelEvent::Usage(TokenUsage {
                input_tokens: 12,
                output_tokens: 3,
                ..Default::default()
            }),
        ]));
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        prompt_titles(&first, &session_id, "hello").await;
        prompt_titles(&first, &session_id, "again").await;

        let second = agent(scripted(vec![]));
        initialize(&second).await;
        let mut rx = second.subscribe();
        let response = result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );

        let turns = response["_meta"]["srud"]["turns"]
            .as_array()
            .expect("the load response lists the turns");
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_ne!(
            turns[0]["turnId"], turns[1]["turnId"],
            "each turn is its own"
        );
        for turn in turns {
            assert_eq!(turn["endReason"], json!("completed"), "{turn:?}");
        }

        // A replayed update is the update live streaming sent. The usage records
        // are not among them: this agent configures no context window.
        let updates = replayed_updates(&mut rx);
        assert_eq!(updates.len(), 4, "{updates:?}");
        assert!(
            updates.iter().all(|(_, end)| end.is_none()),
            "no update closes a turn: {updates:?}"
        );
    }

    #[tokio::test]
    async fn wrong_protocol_version_is_invalid_params() {
        let agent = agent(scripted(vec![]));
        let error = error_of(
            agent
                .handle(request(
                    INITIALIZE,
                    json!({ "protocolVersion": 99, "clientCapabilities": {} }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn session_new_returns_an_opaque_id() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let id = new_session(&agent).await;
        assert!(!id.0.is_empty());
        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        // Counted rather than asserted as the only one: the configuration
        // directory comes from a process-wide environment variable that tests
        // beside this one also set, so the listing may be reading their home.
        assert!(
            listed["sessions"]
                .as_array()
                .is_some_and(|all| !all.is_empty()),
            "the session it just made is listed"
        );
        // The session works in the workspace created for it, named by its id —
        // not in the directory the client sent.
        let mine = listed["sessions"]
            .as_array()
            .expect("a list")
            .iter()
            .find(|info| info["sessionId"] == json!(id.0.as_ref()))
            .expect("the session it just made is listed");
        let cwd = mine["cwd"]
            .as_str()
            .expect("the session reports a working directory");
        assert!(
            cwd.ends_with(id.0.as_ref()),
            "the workspace is named by the session id: {cwd}"
        );
        assert!(
            std::path::Path::new(cwd).is_dir(),
            "the workspace exists: {cwd}"
        );
    }

    #[tokio::test]
    async fn session_new_works_where_the_client_said_to() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let chosen = std::env::temp_dir().join(format!("srud-chosen-{}", std::process::id()));
        std::fs::create_dir_all(&chosen).expect("a directory to work in");

        let reply = agent
            .handle(request(
                SESSION_NEW,
                json!({ "cwd": chosen.to_str(), "mcpServers": [] }),
            ))
            .await;
        let Response::Result { result, .. } = reply.into_inner() else {
            panic!("session/new should succeed");
        };
        let id = SessionId::new(result["sessionId"].as_str().unwrap().to_string());

        let listed = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        assert_eq!(listed["sessions"][0]["cwd"], chosen.to_str().unwrap());

        // A directory the client named is the user's, so deleting the session
        // must leave it standing.
        result_of(
            agent
                .handle(request(
                    SESSION_DELETE,
                    json!({ "sessionId": id.0.as_ref() }),
                ))
                .await,
        );
        assert!(chosen.is_dir(), "{chosen:?} is not the agent's to delete");
    }

    #[tokio::test]
    async fn prompt_streams_updates_and_returns_stop_reason() {
        let agent = agent(scripted(vec![
            ModelEvent::TextDelta { delta: "he".into() },
            ModelEvent::TextDelta {
                delta: "llo".into(),
            },
        ]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let mut rx = agent.subscribe();

        let result = result_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hi" }]
                    }),
                ))
                .await,
        );
        assert_eq!(result["stopReason"], json!("end_turn"));

        let mut kinds = Vec::new();
        while let Ok(notification) = rx.try_recv() {
            let value = serde_json::to_value(&notification).unwrap();
            assert_eq!(value["params"]["sessionId"], json!(session_id.0.as_ref()));
            let kind = value["params"]["update"]["sessionUpdate"]
                .as_str()
                .unwrap()
                .to_string();
            // Session-metadata updates are not turn output, so they carry no
            // turn id; everything the turn loop emits does.
            if kind != "session_info_update" {
                assert!(
                    value["params"]["_meta"]["srud"]["turnId"].is_string(),
                    "{kind} carries a turn id"
                );
            }
            kinds.push(kind);
        }
        assert_eq!(
            kinds,
            vec![
                // The first prompt names the session, before any turn output.
                "session_info_update",
                "user_message_chunk",
                "agent_message_chunk",
                "agent_message_chunk"
            ]
        );
    }

    #[tokio::test]
    async fn cancel_makes_prompt_return_cancelled() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let prompt = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        // Give the turn a moment to claim the slot and emit its first delta.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cancel_reply = agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        assert!(matches!(cancel_reply.inner(), Response::Result { .. }));

        let result = result_of(prompt.await.unwrap());
        assert_eq!(result["stopReason"], json!("cancelled"));
    }

    #[tokio::test]
    async fn shutdown_interrupts_an_active_turn_and_refuses_new_prompts() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let prompt = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        for _ in 0..100 {
            if agent
                .sessions
                .get(&session_id)
                .is_some_and(|session| session.is_busy())
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            agent
                .sessions
                .get(&session_id)
                .is_some_and(|session| session.is_busy()),
            "the turn should have claimed the session before shutdown"
        );

        assert!(
            agent.shutdown(std::time::Duration::from_secs(2)).await,
            "the in-flight turn should settle within the timeout"
        );
        let result = result_of(prompt.await.unwrap());
        assert_eq!(result["stopReason"], json!("cancelled"));

        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "after" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INTERNAL_ERROR);
        assert!(
            error.message.contains("shutting down"),
            "the refusal names why: {error:?}"
        );
    }

    #[tokio::test]
    async fn prompt_again_while_busy_is_session_busy() {
        let model = stalling(vec![ModelEvent::TextDelta {
            delta: "partial".into(),
        }]);
        let agent = Arc::new(agent(model));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let prompt_agent = Arc::clone(&agent);
        let prompt_session = session_id.clone();
        let first = tokio::spawn(async move {
            prompt_agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": prompt_session.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "go" }]
                    }),
                ))
                .await
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "second" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_BUSY);

        // Clean up: cancel so the in-flight turn finishes.
        agent
            .handle(request(
                SESSION_CANCEL,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        let _ = first.await.unwrap();
    }

    #[tokio::test]
    async fn the_first_prompt_names_the_session_and_broadcasts_it() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let titles = prompt_titles(&agent, &session_id, "Why is the timestamp wrong?").await;
        assert_eq!(
            titles,
            vec![Some("Why is the timestamp wrong?".to_string())],
            "the first prompt names the session once"
        );

        // A later turn must not rename it.
        let later = prompt_titles(&agent, &session_id, "A completely different question").await;
        assert!(
            later.is_empty(),
            "a later prompt does not rename the session"
        );
    }

    #[tokio::test]
    async fn set_title_renames_and_broadcasts() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let mut rx = agent.subscribe();
        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": session_id.0.as_ref(), "title": "Chosen by hand" }),
                ))
                .await,
        );
        assert_eq!(result["title"], json!("Chosen by hand"));

        let notification = rx.try_recv().expect("the rename is broadcast");
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["method"], json!("session/update"));
        assert_eq!(
            value["params"]["update"]["sessionUpdate"],
            json!("session_info_update")
        );
        assert_eq!(value["params"]["update"]["title"], json!("Chosen by hand"));

        // A rename outranks the derived name, so the next prompt leaves it alone.
        let titles = prompt_titles(&agent, &session_id, "another question").await;
        assert!(titles.is_empty());
    }

    #[tokio::test]
    async fn a_blank_rename_clears_the_title_and_reports_it() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        agent
            .handle(request(
                SRUD_SESSION_SET_TITLE,
                json!({ "sessionId": session_id.0.as_ref(), "title": "Named" }),
            ))
            .await;

        let mut rx = agent.subscribe();
        let result = result_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": session_id.0.as_ref(), "title": "  " }),
                ))
                .await,
        );
        assert!(
            result.get("title").is_none(),
            "a blank title is reported as the clear it was"
        );

        let notification = rx.try_recv().expect("the clear is broadcast");
        let value = serde_json::to_value(&notification).unwrap();
        assert_eq!(value["params"]["update"]["title"], json!(null));
    }

    #[tokio::test]
    async fn renaming_an_unknown_session_is_session_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SRUD_SESSION_SET_TITLE,
                    json!({ "sessionId": "missing", "title": "x" }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn list_reports_the_derived_title() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        prompt_titles(&agent, &session_id, "Name me").await;

        let result = result_of(agent.handle(request(SESSION_LIST, json!({}))).await);
        let listed = result["sessions"]
            .as_array()
            .expect("sessions is an array")
            .iter()
            .find(|info| info["sessionId"] == json!(session_id.0.as_ref()))
            .expect("the prompted session is listed");
        assert_eq!(listed["title"], json!("Name me"));
    }

    #[tokio::test]
    async fn unknown_and_unimplemented_methods_are_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        for method in UNIMPLEMENTED_METHODS {
            let error = error_of(agent.handle(request(method, json!({}))).await);
            assert_eq!(i32::from(error.code), METHOD_NOT_FOUND, "for {method}");
        }
        let error = error_of(agent.handle(request("bogus/method", json!({}))).await);
        assert_eq!(i32::from(error.code), METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn prompt_on_unknown_session_is_session_not_found() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": "missing",
                        "prompt": [{ "type": "text", "text": "hi" }]
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn close_and_delete_deregister_the_session() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;

        let reply = agent
            .handle(request(
                SESSION_CLOSE,
                json!({ "sessionId": session_id.0.as_ref() }),
            ))
            .await;
        assert!(matches!(reply.inner(), Response::Result { .. }));

        let error = error_of(
            agent
                .handle(request(
                    SESSION_DELETE,
                    json!({ "sessionId": session_id.0.as_ref() }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), SESSION_NOT_FOUND);
    }

    #[tokio::test]
    async fn malformed_params_are_invalid_params() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let error = error_of(
            agent
                .handle(request(SESSION_NEW, json!({ "notCwd": 1 })))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn audio_prompt_is_invalid_params() {
        let agent = agent(scripted(vec![ModelEvent::TextDelta { delta: "ok".into() }]));
        initialize(&agent).await;
        let session_id = new_session(&agent).await;
        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Audio(srud_protocol::acp::AudioContent::new("data", "audio/wav")),
        ];
        let params = json!({
            "sessionId": session_id.0.as_ref(),
            "prompt": serde_json::to_value(&blocks).unwrap(),
        });
        let error = error_of(agent.handle(request(SESSION_PROMPT, params)).await);
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[test]
    fn prompt_parts_accepts_text_and_images_and_rejects_audio() {
        let blocks = vec![
            ContentBlock::Text(TextContent::new("hello")),
            ContentBlock::Image(srud_protocol::acp::ImageContent::new("d", "image/png")),
        ];
        let parts = prompt_parts(&blocks).unwrap();
        assert_eq!(parts.text, "hello");
        assert_eq!(
            parts.images,
            vec![srud_core::types::ImagePart {
                mime: "image/png".into(),
                data: "d".into(),
            }]
        );

        let blocks = vec![
            ContentBlock::Text(TextContent::new("a")),
            ContentBlock::Audio(srud_protocol::acp::AudioContent::new("d", "audio/wav")),
        ];
        let err = prompt_parts(&blocks).unwrap_err();
        assert_eq!(i32::from(err.code), INVALID_PARAMS);
        assert!(err.message.contains("Audio"));
    }

    #[test]
    fn reply_helpers_produce_well_formed_envelopes() {
        let ok = reply_ok(RequestId::Number(3), json!({ "a": 1 }));
        let value = serde_json::to_value(&ok).unwrap();
        assert_eq!(value["jsonrpc"], json!("2.0"));
        assert_eq!(value["id"], json!(3));
        assert_eq!(value["result"]["a"], json!(1));

        let err = reply_err(
            RequestId::Str("x".into()),
            AcpError::new(INTERNAL_ERROR, "boom"),
        );
        let value = serde_json::to_value(&err).unwrap();
        assert_eq!(value["error"]["code"], json!(INTERNAL_ERROR));
    }

    // ---- the configurable-model path --------------------------------------

    /// A factory that records the references it was asked to build and answers
    /// every one of them with the same scripted client.
    struct RecordingFactory {
        events: Vec<ModelEvent>,
        built: std::sync::Mutex<Vec<String>>,
    }

    impl RecordingFactory {
        fn new(events: Vec<ModelEvent>) -> Arc<Self> {
            Arc::new(Self {
                events,
                built: std::sync::Mutex::new(Vec::new()),
            })
        }

        /// The references built so far, in order.
        fn built(&self) -> Vec<String> {
            self.built.lock().expect("factory lock poisoned").clone()
        }
    }

    impl ModelFactory for RecordingFactory {
        fn build(
            &self,
            provider: &Provider,
            model: &str,
        ) -> Result<Arc<dyn ModelClient>, srud_core::client::ModelError> {
            self.built
                .lock()
                .expect("factory lock poisoned")
                .push(format!("{}/{}", provider.id, model));
            Ok(Arc::new(Scripted {
                events: self.events.clone(),
                stall_until_cancel: false,
            }))
        }
    }

    fn sample_provider(id: &str, models: &[&str]) -> ProviderInput {
        ProviderInput {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            base_url: format!("https://{id}.example/v1"),
            api_key: Some(format!("sk-{id}-secret-key")),
            models: models
                .iter()
                .map(|model| Model {
                    id: (*model).to_owned(),
                    name: None,
                    modalities: Vec::new(),
                    context_window: None,
                })
                .collect(),
        }
    }

    /// An agent over a two-provider store, and the factory it resolves with.
    ///
    /// The configuration directory is handed back before this returns: the store
    /// has already read the file, and holding the environment across the caller's
    /// own session setup would deadlock two guards against each other.
    fn providers_agent(events: Vec<ModelEvent>) -> (Agent, Arc<RecordingFactory>) {
        providers_agent_with(events, true)
    }

    /// As [`providers_agent`], but `fixed` decides whether the host also left a
    /// client behind for sessions that name no model. A host with nothing but a
    /// provider file leaves none.
    fn providers_agent_with(
        events: Vec<ModelEvent>,
        fixed: bool,
    ) -> (Agent, Arc<RecordingFactory>) {
        let (store, factory) = {
            let _env = Guard::take(&[config::HOME_VAR]);
            std::env::set_var(
                config::HOME_VAR,
                test_env::unique_dir("srud-agent-providers"),
            );
            let store = ProviderStore::load().expect("an empty store");
            store
                .upsert(sample_provider("openai", &["small", "large"]))
                .expect("a valid provider");
            store
                .upsert(sample_provider("anthropic", &["opus"]))
                .expect("a valid provider");
            (store, RecordingFactory::new(events))
        };
        let model = if fixed {
            Some(scripted(Vec::new()) as Arc<dyn ModelClient>)
        } else {
            None
        };
        let agent = Agent::with_providers(
            model,
            Arc::new(ToolRegistry::new()),
            ModelProviders::new(Arc::new(store), factory.clone()),
        );
        (agent, factory)
    }

    /// Points the configuration directory at one of this test's own, so a session
    /// is created under the system temp directory rather than the developer's
    /// home. Kept alive for the length of the test.
    fn home() -> Guard {
        let guard = Guard::take(&[config::HOME_VAR]);
        std::env::set_var(config::HOME_VAR, test_env::unique_dir("srud-agent-home"));
        guard
    }

    async fn prompt_once(agent: &Agent, session_id: &SessionId) {
        result_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hello" }],
                    }),
                ))
                .await,
        );
    }

    async fn choose(agent: &Agent, session_id: &SessionId, reference: &str) -> Value {
        result_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "model",
                        "value": reference,
                    }),
                ))
                .await,
        )
    }

    async fn new_session_result(agent: &Agent) -> Value {
        result_of(
            agent
                .handle(request(SESSION_NEW, json!({ "cwd": "", "mcpServers": [] })))
                .await,
        )
    }

    #[tokio::test]
    async fn a_new_session_offers_the_configured_models() {
        let (agent, _factory) = providers_agent(vec![]);
        initialize(&agent).await;
        let _home = home();

        let result = new_session_result(&agent).await;
        let options = result["configOptions"]
            .as_array()
            .expect("a model selector");
        assert_eq!(options.len(), 1, "the model option is the only one offered");
        let option = &options[0];
        assert_eq!(option["id"], json!("model"));
        assert_eq!(option["category"], json!("model"));
        assert_eq!(option["type"], json!("select"));
        // Nobody has chosen a default, so nothing is chosen for the session
        // either. The empty value is what the picker renders as "choose one".
        assert_eq!(option["currentValue"], json!(""));

        let groups = option["options"].as_array().expect("grouped by provider");
        let named: Vec<&str> = groups
            .iter()
            .map(|group| group["name"].as_str().unwrap())
            .collect();
        assert_eq!(named, ["Provider openai", "Provider anthropic"]);
        assert_eq!(groups[0]["group"], json!("openai"));
        let values: Vec<&str> = groups[0]["options"]
            .as_array()
            .expect("a group's options")
            .iter()
            .map(|value| value["value"].as_str().unwrap())
            .collect();
        assert_eq!(values, ["openai/small", "openai/large"]);
    }

    #[tokio::test]
    async fn a_new_session_starts_on_the_chosen_default() {
        let (agent, factory) = providers_agent(vec![]);
        initialize(&agent).await;
        let _home = home();
        // Chosen through the panel's own method, which is the only writer of it.
        result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_SET_DEFAULT,
                    json!({ "model": "anthropic/opus" }),
                ))
                .await,
        );

        let result = new_session_result(&agent).await;
        assert_eq!(
            result["configOptions"][0]["currentValue"],
            json!("anthropic/opus")
        );

        let session_id = SessionId::new(result["sessionId"].as_str().unwrap());
        prompt_once(&agent, &session_id).await;
        assert_eq!(factory.built(), vec!["anthropic/opus"]);
    }

    #[tokio::test]
    async fn a_session_with_no_model_and_no_fallback_is_told_to_choose_one() {
        let (agent, factory) = providers_agent_with(vec![], false);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hello" }],
                    }),
                ))
                .await,
        );

        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        assert!(
            error.message.contains("choose one"),
            "the message says what to do: {}",
            error.message
        );
        assert!(factory.built().is_empty(), "nothing ran");
    }

    #[tokio::test]
    async fn a_session_with_no_model_runs_on_the_client_the_host_fixed() {
        // A log written before the provider file existed names no model. It runs
        // on what it ran on then rather than on a model chosen since.
        let (agent, factory) =
            providers_agent_with(vec![ModelEvent::TextDelta { delta: "ok".into() }], true);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        prompt_once(&agent, &session_id).await;
        assert!(factory.built().is_empty(), "no provider was asked for one");
    }

    #[tokio::test]
    async fn a_chosen_model_runs_the_turn_and_is_broadcast() {
        let (agent, factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        let mut rx = agent.subscribe();
        let result = choose(&agent, &session_id, "anthropic/opus").await;
        // The reply carries the whole set, so the client renders its own change.
        assert_eq!(
            result["configOptions"][0]["currentValue"],
            json!("anthropic/opus")
        );

        let update = serde_json::to_value(rx.try_recv().expect("the change is announced")).unwrap();
        assert_eq!(
            update["params"]["update"]["sessionUpdate"],
            json!("config_option_update")
        );
        assert_eq!(
            update["params"]["update"]["configOptions"][0]["currentValue"],
            json!("anthropic/opus")
        );
        assert_eq!(update["params"]["sessionId"], json!(session_id.0.as_ref()));

        prompt_once(&agent, &session_id).await;
        assert_eq!(factory.built(), vec!["anthropic/opus"]);
    }

    #[tokio::test]
    async fn an_unknown_model_is_refused_and_the_session_keeps_its_own() {
        let (agent, factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;
        // Chosen first, so there is an "own" for the refusal to leave alone.
        result_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "model",
                        "value": "openai/small",
                    }),
                ))
                .await,
        );

        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "model",
                        "value": "openai/missing",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);

        // Refused, so the session still runs on what it was seeded with.
        prompt_once(&agent, &session_id).await;
        assert_eq!(factory.built(), vec!["openai/small"]);
    }

    #[tokio::test]
    async fn setting_an_option_that_does_not_exist_is_invalid_params() {
        let (agent, _factory) = providers_agent(vec![]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;

        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "configId": "mode",
                        "value": "anthropic/opus",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
    }

    #[tokio::test]
    async fn a_provider_deleted_under_a_session_makes_its_turns_refuse() {
        let (agent, _factory) = providers_agent(vec![ModelEvent::TextDelta { delta: "ok".into() }]);
        initialize(&agent).await;
        let _home = home();
        let session_id = new_session_in(&agent, "").await;
        choose(&agent, &session_id, "anthropic/opus").await;

        // The provider goes away, as when a user removes it from the panel.
        result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_DELETE,
                    json!({ "providerId": "anthropic" }),
                ))
                .await,
        );

        // Refused rather than quietly run on another provider: a turn recorded
        // under one model while another answered would make the log a lie.
        let error = error_of(
            agent
                .handle(request(
                    SESSION_PROMPT,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "prompt": [{ "type": "text", "text": "hello" }],
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), INVALID_PARAMS);
        assert!(
            error.message.contains("no longer configured"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn provider_methods_round_trip_over_rpc() {
        let (agent, _factory) = providers_agent(vec![]);
        initialize(&agent).await;

        let listed = result_of(agent.handle(request(SRUD_PROVIDER_LIST, json!({}))).await);
        assert_eq!(listed["providers"].as_array().unwrap().len(), 2);
        assert!(listed["default"].is_null(), "nothing is the default yet");
        // A key is readable back only as a mask — enough to tell two apart, not
        // enough to use one.
        assert_eq!(listed["providers"][0]["apiKey"], json!("sk-…-key"));
        assert_eq!(listed["providers"][0]["hasKey"], json!(true));

        let added = result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_SET,
                    json!({
                        "provider": {
                            "id": "local",
                            "name": "Local",
                            "baseUrl": "https://local.example/v1",
                            "apiKey": "sk-local-1234567890",
                            "models": [{ "id": "m", "name": "M" }],
                        }
                    }),
                ))
                .await,
        );
        assert_eq!(added["providers"].as_array().unwrap().len(), 3);

        let defaulted = result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_SET_DEFAULT,
                    json!({ "model": "openai/large" }),
                ))
                .await,
        );
        assert_eq!(defaulted["default"], json!("openai/large"));

        // And that is where the next session starts.
        let _home = home();
        assert_eq!(
            new_session_result(&agent).await["configOptions"][0]["currentValue"],
            json!("openai/large")
        );

        let removed = result_of(
            agent
                .handle(request(
                    SRUD_PROVIDER_DELETE,
                    json!({ "providerId": "local" }),
                ))
                .await,
        );
        assert_eq!(removed["providers"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_agent_without_providers_offers_no_model_option() {
        let agent = agent(scripted(vec![]));
        initialize(&agent).await;
        let _home = home();

        let result = new_session_result(&agent).await;
        assert!(
            result["configOptions"].is_null(),
            "a fixed client has nothing to choose between"
        );

        // The method is not merely unpopulated — it is refused outright, matching
        // what the capabilities promised.
        let error = error_of(
            agent
                .handle(request(
                    SESSION_SET_CONFIG_OPTION,
                    json!({
                        "sessionId": result["sessionId"].as_str().unwrap(),
                        "configId": "model",
                        "value": "openai/small",
                    }),
                ))
                .await,
        );
        assert_eq!(i32::from(error.code), METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn a_restored_session_remembers_the_model_it_chose() {
        let _env = Guard::take(&[config::HOME_VAR]);
        std::env::set_var(config::HOME_VAR, test_env::unique_dir("srud-agent-restore"));
        let store = Arc::new(ProviderStore::load().expect("an empty store"));
        store
            .upsert(sample_provider("openai", &["small", "large"]))
            .unwrap();
        store
            .upsert(sample_provider("anthropic", &["opus"]))
            .unwrap();

        let factory = RecordingFactory::new(vec![ModelEvent::TextDelta { delta: "hi".into() }]);
        let first = Agent::with_providers(
            Some(scripted(Vec::new())),
            Arc::new(ToolRegistry::new()),
            ModelProviders::new(store.clone(), factory),
        );
        initialize(&first).await;
        let (session_id, cwd) = session_with_a_home(&first).await;
        choose(&first, &session_id, "anthropic/opus").await;
        prompt_once(&first, &session_id).await;
        drop(first);

        // A second agent, as after a restart, rebuilt from the same log.
        let second = Agent::with_providers(
            Some(scripted(Vec::new())),
            Arc::new(ToolRegistry::new()),
            ModelProviders::new(store, RecordingFactory::new(Vec::new())),
        );
        initialize(&second).await;
        let result = result_of(
            second
                .handle(request(
                    SESSION_LOAD,
                    json!({
                        "sessionId": session_id.0.as_ref(),
                        "cwd": cwd,
                        "mcpServers": [],
                    }),
                ))
                .await,
        );
        assert_eq!(
            result["configOptions"][0]["currentValue"],
            json!("anthropic/opus"),
            "the choice survives the restart"
        );
    }
}
