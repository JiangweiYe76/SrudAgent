//! Provider and model configuration.
//!
//! One file holds every provider the user has configured: the endpoint, the
//! credentials for it, and the models it offers. It lives under the
//! configuration directory as `providers.json` — a file the user may edit by
//! hand, which is why it is separate from the session logs the agent writes.
//!
//! # Why this is not a `configOptions` concern
//!
//! ACP's `configOptions` carry what a *session* may choose between, and their
//! values are plain strings. Credentials are neither per-session nor a string a
//! client should ever see, so they stay here: [`ProviderStore`] is read and
//! written by the agent's own `_srud/unstable/provider/*` methods, and what a
//! session sees is only the `providerId/modelId` reference it selected.
//!
//! # What a model reference is
//!
//! `providerId/modelId`. The provider is part of it because the same model id
//! (`gpt-4o`) is offered by several providers, so the id alone names nothing.
//! `/` is not in either id's character set, which is what makes the split
//! unambiguous.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use srud_core::client::{ModelClient, ModelError};
use srud_protocol::acp::{
    SessionConfigOption, SessionConfigSelectGroup, SessionConfigSelectOption,
    SessionConfigSelectOptions,
};
use srud_protocol::srud::methods::{Modality, ProviderModel, ProviderView};

use crate::config;

/// The one file format this build reads and writes.
///
/// A file written at another version is refused rather than guessed at: the
/// records it holds would be interpreted by rules that may no longer apply, and
/// a misread credential file is worse than a refused one.
pub const FORMAT_VERSION: u32 = 1;

/// The id of the session config option that selects a model.
pub const MODEL_OPTION_ID: &str = "model";

/// The label the model selector is shown under.
const MODEL_OPTION_NAME: &str = "Model";

/// One model a provider offers.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Model {
    /// The id sent to the provider.
    pub id: String,
    /// What the model is called in a menu. Absent means the id is the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the model accepts besides text. Empty is a text-only model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modalities: Vec<Modality>,
    /// Maximum context window in tokens, as configured by the user. Used as
    /// `UsageUpdate.size` so clients can render context pressure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

impl Model {
    /// What to show for this model.
    #[must_use]
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }
}

/// One provider: an endpoint and the credentials for it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Provider {
    /// Stable identifier, used in model references. Unique across providers.
    pub id: String,
    /// Human-readable label.
    pub name: String,
    /// Endpoint root, with no trailing slash, including whatever version
    /// segment the provider expects.
    pub base_url: String,
    /// Bearer token. Empty for a local endpoint that wants none.
    #[serde(default)]
    pub api_key: String,
    /// The models this provider offers. May be empty while one is being set
    /// up: a provider is added first and has models added to it afterwards.
    pub models: Vec<Model>,
}

/// A provider as a write request describes it.
///
/// Separate from [`Provider`] because `api_key` means something different here:
/// a stored provider always *has* a key string (possibly empty), while a request
/// may say nothing about the key at all, and that is not the same as clearing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderInput {
    /// See [`Provider::id`].
    pub id: String,
    /// See [`Provider::name`].
    pub name: String,
    /// See [`Provider::base_url`].
    pub base_url: String,
    /// The new key. `None` keeps whatever is stored; `Some("")` clears it.
    pub api_key: Option<String>,
    /// See [`Provider::models`].
    pub models: Vec<Model>,
}

/// Everything the provider file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Providers {
    /// The model reference new sessions start from. `None` means no model has
    /// been chosen yet — nothing is inferred from the order providers are listed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// The configured providers, in the order the file lists them.
    #[serde(default)]
    pub providers: Vec<Provider>,
}

impl Providers {
    /// The provider with that id, if it is configured.
    #[must_use]
    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|provider| provider.id == id)
    }

    /// The model reference a new session should start from, if one was chosen.
    ///
    /// `None` when nobody has chosen, and also when the stored `default` names a
    /// provider or model that is no longer there — a file edited by hand can
    /// leave one behind. Nothing is inferred from the order the file happens to
    /// list providers in: a session with no model says so, and the picker is
    /// where one is chosen.
    #[must_use]
    pub fn start_reference(&self) -> Option<String> {
        let default = self.default.as_deref()?;
        self.resolve(default).ok()?;
        Some(default.to_owned())
    }

    /// The provider and model a reference names.
    ///
    /// # Errors
    ///
    /// [`ProviderError::BadReference`] when the reference is not
    /// `provider/model`, [`ProviderError::UnknownProvider`] when no provider has
    /// that id, and [`ProviderError::UnknownModel`] when that provider does not
    /// offer the model.
    pub fn resolve(&self, reference: &str) -> Result<(&Provider, &Model), ProviderError> {
        let (provider_id, model_id) = split_reference(reference)
            .ok_or_else(|| ProviderError::BadReference(reference.to_owned()))?;
        let provider = self
            .provider(provider_id)
            .ok_or_else(|| ProviderError::UnknownProvider(provider_id.to_owned()))?;
        let model = provider
            .models
            .iter()
            .find(|model| model.id == model_id)
            .ok_or_else(|| ProviderError::UnknownModel(reference.to_owned()))?;
        Ok((provider, model))
    }
}

/// Why a provider document, or a change to one, was refused.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// There is no configuration directory to put the file in.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// The file could not be read or written.
    #[error("cannot use {path}: {source}")]
    Io {
        /// The file that could not be used.
        path: PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },

    /// The file does not hold what this build understands.
    #[error("{path} is not a provider file this build can read: {message}")]
    Parse {
        /// The file that could not be read.
        path: PathBuf,
        /// What was wrong with it.
        message: String,
    },

    /// A provider was rejected.
    #[error("invalid provider: {0}")]
    Invalid(String),

    /// No configured provider has that id.
    #[error("no provider with id `{0}`")]
    UnknownProvider(String),

    /// That provider does not offer that model.
    #[error("no model `{0}`")]
    UnknownModel(String),

    /// The reference is not `provider/model`.
    #[error("`{0}` is not a `provider/model` reference")]
    BadReference(String),
}

/// Whether an id is usable as one half of a model reference.
///
/// `[a-z0-9][a-z0-9-_]*`: lowercase, so two ids differing only in case cannot
/// both be configured into a reference that reads as one; and no `/`, which is
/// the separator.
fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_lowercase() || first.is_ascii_digit() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Whether a base URL is one this agent will send a key to.
///
/// Absolute `http` or `https` only. A relative value names a different host
/// depending on who resolves it — the same reason [`config::home`] refuses one.
/// `http` is allowed so a local endpoint can be pointed at directly.
fn valid_base_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    if rest.is_empty() || url.ends_with('/') {
        return false;
    }
    if rest.chars().any(char::is_whitespace) {
        return false;
    }
    // The authority must be there and must not itself be a path.
    rest.split('/').next().is_some_and(|host| !host.is_empty())
}

/// Builds a model reference from its two halves.
fn reference(provider_id: &str, model_id: &str) -> String {
    format!("{provider_id}/{model_id}")
}

/// Splits a model reference into its provider and model halves.
fn split_reference(reference: &str) -> Option<(&str, &str)> {
    let (provider_id, model_id) = reference.split_once('/')?;
    let usable = !provider_id.is_empty()
        && !model_id.is_empty()
        && !provider_id.contains('/')
        && !model_id.contains('/');
    usable.then_some((provider_id, model_id))
}

/// How a key reads once it has been masked for display.
///
/// Shows enough to tell two keys apart and not enough to use one. A key too
/// short to halve shows nothing of itself at all.
fn mask(api_key: &str) -> Option<String> {
    if api_key.is_empty() {
        return None;
    }
    let chars: Vec<char> = api_key.chars().collect();
    if chars.len() <= 8 {
        return Some("…".to_owned());
    }
    let head: String = chars[..3].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    Some(format!("{head}…{tail}"))
}

/// The provider file, and the copy of it held in memory.
///
/// Read once and kept, so a model reference is resolved per turn without
/// touching the disk. Writes go through the store, so the file and the copy
/// cannot disagree about what the user just configured.
///
/// The lock is a plain `RwLock` and the file operations are synchronous: this
/// file is a few kilobytes and is written when the user edits a setting, not on
/// any path a turn takes.
#[derive(Debug)]
pub struct ProviderStore {
    path: PathBuf,
    inner: RwLock<Providers>,
}

impl ProviderStore {
    /// Reads the provider file, or starts empty when there is none.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Config`] when there is no configuration directory to
    /// look in, [`ProviderError::Io`] when the file cannot be read, and
    /// [`ProviderError::Parse`] when it holds something this build cannot
    /// interpret. A missing file is not an error: it means nothing is
    /// configured yet.
    pub fn load() -> Result<Self, ProviderError> {
        let path = config::providers_path()?;
        let providers = read(&path)?;
        Ok(Self {
            path,
            inner: RwLock::new(providers),
        })
    }

    /// Creates a store over a specific file, reading it now.
    ///
    /// For a caller that names the file itself rather than taking the default:
    /// a test, or a host with its own configuration source.
    pub fn with_path(path: impl Into<PathBuf>) -> Result<Self, ProviderError> {
        let path = path.into();
        let providers = read(&path)?;
        Ok(Self {
            path,
            inner: RwLock::new(providers),
        })
    }

    /// An empty store that writes to `path` on its first edit.
    ///
    /// For a host that must run without a readable provider file: the app stays
    /// usable, and the file it could not read is replaced only once the user
    /// actually saves something.
    #[must_use]
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            inner: RwLock::new(Providers::default()),
        }
    }

    /// The file this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A copy of everything the store holds.
    #[must_use]
    pub fn snapshot(&self) -> Providers {
        self.inner.read().expect("provider lock poisoned").clone()
    }

    /// The model reference a new session should start from.
    #[must_use]
    pub fn start_reference(&self) -> Option<String> {
        self.snapshot().start_reference()
    }

    /// The provider and model a reference names, as a snapshot.
    ///
    /// # Errors
    ///
    /// As [`Providers::resolve`].
    pub fn resolve(&self, reference: &str) -> Result<(Provider, Model), ProviderError> {
        let providers = self.inner.read().expect("provider lock poisoned");
        let (provider, model) = providers.resolve(reference)?;
        Ok((provider.clone(), model.clone()))
    }

    /// Adds a provider, or replaces the one with the same id.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Invalid`] when the provider does not pass validation, or
    /// [`ProviderError::Io`] when the change cannot be written — in which case
    /// nothing is applied, so what a caller reads back is what is on disk.
    pub fn upsert(&self, input: ProviderInput) -> Result<Providers, ProviderError> {
        let mut providers = self.inner.write().expect("provider lock poisoned");

        let existing = providers.provider(&input.id).cloned();
        let provider = Provider {
            id: input.id.clone(),
            name: input.name.trim().to_owned(),
            base_url: input.base_url.trim().trim_end_matches('/').to_owned(),
            // The request's silence is what "keep it" means: a panel that read a
            // masked key back and saved it unchanged must not overwrite the real
            // one with its own display value.
            api_key: match input.api_key {
                Some(key) => key.trim().to_owned(),
                None => existing
                    .as_ref()
                    .map(|p| p.api_key.clone())
                    .unwrap_or_default(),
            },
            // The id is trimmed here, not only checked trimmed: validation
            // compares trimmed values, and a stored reference has to be the one
            // `resolve` later looks up.
            models: input
                .models
                .into_iter()
                .map(|model| Model {
                    id: model.id.trim().to_owned(),
                    ..model
                })
                .collect(),
        };
        validate_provider(&provider)?;

        let next = match providers.providers.iter().position(|p| p.id == provider.id) {
            Some(index) => {
                let mut next = (*providers).clone();
                next.providers[index] = provider;
                next
            }
            None => {
                let mut next = (*providers).clone();
                next.providers.push(provider);
                next
            }
        };

        write(&self.path, &next)?;
        *providers = next;
        Ok(providers.clone())
    }

    /// Removes a provider.
    ///
    /// A `default` naming one of its models is cleared with it, rather than
    /// moved to another provider: silently starting new sessions on a model the
    /// user did not pick is a change they did not ask for.
    ///
    /// # Errors
    ///
    /// [`ProviderError::UnknownProvider`] when no provider has that id, and
    /// [`ProviderError::Io`] when the removal cannot be written — in which case
    /// nothing is applied.
    pub fn delete(&self, provider_id: &str) -> Result<Providers, ProviderError> {
        let mut providers = self.inner.write().expect("provider lock poisoned");
        if providers.provider(provider_id).is_none() {
            return Err(ProviderError::UnknownProvider(provider_id.to_owned()));
        }

        let mut next = (*providers).clone();
        next.providers.retain(|provider| provider.id != provider_id);
        if next
            .default
            .as_deref()
            .is_some_and(|default| next.resolve(default).is_err())
        {
            next.default = None;
        }

        write(&self.path, &next)?;
        *providers = next;
        Ok(providers.clone())
    }

    /// Sets the model new sessions start from.
    ///
    /// The only writer of `default`. Switching a model inside a session goes
    /// through `session/set_config_option`, which does not touch this — a
    /// session's choice is its own, and ACP has no way to express "change it
    /// everywhere".
    ///
    /// # Errors
    ///
    /// [`ProviderError::BadReference`], [`ProviderError::UnknownProvider`] or
    /// [`ProviderError::UnknownModel`] when the reference names nothing
    /// configured, and [`ProviderError::Io`] when the change cannot be written.
    pub fn set_default(&self, reference: &str) -> Result<Providers, ProviderError> {
        let mut providers = self.inner.write().expect("provider lock poisoned");
        providers.resolve(reference)?;

        let mut next = (*providers).clone();
        next.default = Some(reference.to_owned());
        write(&self.path, &next)?;
        *providers = next;
        Ok(providers.clone())
    }

    /// The model selector, as a session is offered it.
    ///
    /// Grouped by provider, since that is the two-level shape the choice
    /// actually has. Returns `None` when nothing is configured: a session with
    /// no model to pick from is better off with no selector at all than one
    /// offering an empty list.
    ///
    /// `current` is the session's own selection. It is shown even when it no
    /// longer resolves — a provider the user has since deleted — so that a
    /// session whose model has gone is visibly in that state rather than
    /// silently reading as some other model.
    #[must_use]
    pub fn session_config_options(
        &self,
        current: Option<&str>,
    ) -> Option<Vec<SessionConfigOption>> {
        let providers = self.inner.read().expect("provider lock poisoned");
        if providers.providers.is_empty() {
            return None;
        }
        let mut groups: Vec<SessionConfigSelectGroup> = providers
            .providers
            .iter()
            .filter(|provider| !provider.models.is_empty())
            .map(|provider| {
                SessionConfigSelectGroup::new(
                    provider.id.clone(),
                    provider.name.clone(),
                    provider
                        .models
                        .iter()
                        .map(|model| {
                            SessionConfigSelectOption::new(
                                reference(&provider.id, &model.id),
                                model.label(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();

        // No provider offers a model, so there is nothing to pick from and no
        // selector to offer. A provider is allowed to exist before its models
        // are — it is added first, and models are added to it after.
        if groups.is_empty() {
            return None;
        }

        // A selection that no longer resolves is kept as the current value, and
        // added as its own group so the client can render it. Without this the
        // selector would show a value that is not in its own list.
        let known = groups.iter().any(|group| {
            group
                .options
                .iter()
                .any(|option| option.value.0.as_ref() == current.unwrap_or_default())
        });
        if let Some(current) = current {
            if !known {
                groups.push(SessionConfigSelectGroup::new(
                    "unavailable",
                    "Unavailable",
                    vec![SessionConfigSelectOption::new(
                        current.to_owned(),
                        current.to_owned(),
                    )],
                ));
            }
        }

        // The session's own choice, or nothing. An empty value is how "no model
        // chosen yet" travels: the client renders its placeholder, and picking
        // one is the first thing its user does.
        let current_value = current.map(ToOwned::to_owned).unwrap_or_default();

        let mut option = SessionConfigOption::select(
            MODEL_OPTION_ID,
            MODEL_OPTION_NAME,
            current_value,
            SessionConfigSelectOptions::Grouped(groups),
        );
        option.description = Some("The provider and model this conversation uses.".to_owned());
        option.category = Some(srud_protocol::acp::SessionConfigOptionCategory::Model);
        Some(vec![option])
    }

    /// The providers as the wire shows them: keys masked, never sent whole.
    #[must_use]
    pub fn views(&self) -> Vec<ProviderView> {
        self.inner
            .read()
            .expect("provider lock poisoned")
            .providers
            .iter()
            .map(|provider| ProviderView {
                id: provider.id.clone(),
                name: provider.name.clone(),
                base_url: provider.base_url.clone(),
                api_key: mask(&provider.api_key),
                has_key: !provider.api_key.is_empty(),
                models: provider
                    .models
                    .iter()
                    .map(|model| ProviderModel {
                        id: model.id.clone(),
                        name: model.name.clone(),
                        modalities: model.modalities.clone(),
                        context_window: model.context_window,
                    })
                    .collect(),
            })
            .collect()
    }

    /// The model reference new sessions start from, as the wire reports it.
    #[must_use]
    pub fn default_reference(&self) -> Option<String> {
        self.inner
            .read()
            .expect("provider lock poisoned")
            .default
            .clone()
    }
}

/// Builds a model client for one configured provider.
///
/// Implemented by the host, not here: which SDK talks to a provider is the
/// host's decision, and this crate does not depend on one. See the module notes
/// in `crate::lib` for why the model client is the host's to supply.
pub trait ModelFactory: Send + Sync {
    /// Builds a client that sends `model` to `provider`'s endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`ModelError::Config`] when the settings cannot make a client —
    /// an empty key for an endpoint that needs one, above all.
    fn build(&self, provider: &Provider, model: &str) -> Result<Arc<dyn ModelClient>, ModelError>;
}

/// Checks a provider is one this agent can talk to.
fn validate_provider(provider: &Provider) -> Result<(), ProviderError> {
    if !valid_id(&provider.id) {
        return Err(ProviderError::Invalid(format!(
            "id `{}` must be lowercase letters, digits, `-` or `_`, and start with a letter or digit",
            provider.id
        )));
    }
    if provider.name.is_empty() {
        return Err(ProviderError::Invalid("name must not be blank".to_owned()));
    }
    if !valid_base_url(&provider.base_url) {
        return Err(ProviderError::Invalid(format!(
            "baseUrl `{}` must be an absolute http or https URL with no trailing slash",
            provider.base_url
        )));
    }
    // An empty `models` is allowed: a provider is added first and has models
    // added to it afterwards, and a half-filled one is not a broken file. It
    // offers nothing to pick, which is what the selector checks for itself.

    let mut seen = std::collections::BTreeSet::new();
    for model in &provider.models {
        let id = model.id.trim();
        if id.is_empty() {
            return Err(ProviderError::Invalid(
                "a model id must not be blank".to_owned(),
            ));
        }
        if id.contains('/') {
            return Err(ProviderError::Invalid(format!(
                "model id `{id}` must not contain `/`, which separates a reference from its provider"
            )));
        }
        if !seen.insert(id) {
            return Err(ProviderError::Invalid(format!(
                "model id `{id}` is listed twice"
            )));
        }
    }
    Ok(())
}

/// Checks a whole document as a stored file describes it.
///
/// Stricter than a single provider's check because a file is read as one thing:
/// two providers sharing an id would make every reference to it ambiguous, and
/// nothing later could tell which one was meant.
fn validate_document(providers: &Providers) -> Result<(), ProviderError> {
    let mut seen = std::collections::BTreeSet::new();
    for provider in &providers.providers {
        validate_provider(provider)?;
        if !seen.insert(provider.id.as_str()) {
            return Err(ProviderError::Invalid(format!(
                "provider id `{}` is used twice",
                provider.id
            )));
        }
    }
    // A `default` that names nothing is tolerated rather than refused: the file
    // is user-editable, and a dangling reference should cost the default, not
    // every provider in the file. `start_reference` answers `None` when it sees
    // one, which is the same answer it gives when nothing has been chosen.
    Ok(())
}

/// Reads a provider file.
///
/// A file that is not there is an empty document: nothing is configured yet.
fn read(path: &Path) -> Result<Providers, ProviderError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Providers::default())
        }
        Err(source) => {
            return Err(ProviderError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    let raw: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| ProviderError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    let version = raw.get("version").and_then(serde_json::Value::as_u64);
    if version != Some(u64::from(FORMAT_VERSION)) {
        return Err(ProviderError::Parse {
            path: path.to_path_buf(),
            message: format!("format version {version:?}, expected {FORMAT_VERSION}"),
        });
    }

    let providers: Providers =
        serde_json::from_value(raw).map_err(|error| ProviderError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
    validate_document(&providers).map_err(|error| ProviderError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    Ok(providers)
}

/// Writes a provider file, readable only by its owner.
///
/// The keys in it are credentials, so the file the write lands in is the mode a
/// credential file belongs in: `0600` on unix, set explicitly rather than only
/// at creation, so a file that already existed with looser permissions is
/// tightened rather than left as it was.
fn write(path: &Path, providers: &Providers) -> Result<(), ProviderError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| ProviderError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    // The version is written from the constant rather than carried in the type:
    // what a file says it is can only be what this build writes.
    let mut document = serde_json::to_value(providers).map_err(|error| ProviderError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    document["version"] = serde_json::Value::from(FORMAT_VERSION);
    let text = serde_json::to_string_pretty(&document).map_err(|error| ProviderError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;

    // Written to a sibling temporary file and renamed over the target, so a
    // crash mid-write leaves the previous file whole rather than a half-written
    // credential document. Every writer holds the store's lock across this call,
    // so the temporary name cannot collide inside one process.
    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp = PathBuf::from(&temp_name);

    let written = std::fs::write(&temp, &text)
        .map_err(|source| ProviderError::Io {
            path: temp.clone(),
            source,
        })
        .and_then(|()| restrict(&temp))
        .and_then(|()| {
            std::fs::rename(&temp, path).map_err(|source| ProviderError::Io {
                path: path.to_path_buf(),
                source,
            })
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// Makes a file readable only by its owner.
#[cfg(unix)]
fn restrict(path: &Path) -> Result<(), ProviderError> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|source| {
        ProviderError::Io {
            path: path.to_path_buf(),
            source,
        }
    })
}

/// Makes a file readable only by its owner.
///
/// Nothing to do off unix: the platform carries this in the parent directory's
/// ACL rather than in a mode bit.
#[cfg(not(unix))]
fn restrict(_path: &Path) -> Result<(), ProviderError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::test_env::{self, Guard};

    /// A store over a file of its own under the system temp directory.
    fn store() -> (ProviderStore, PathBuf, Guard) {
        let guard = Guard::take(&[config::HOME_VAR]);
        let dir = test_env::unique_dir("srud-providers");
        std::env::set_var(config::HOME_VAR, &dir);
        let path = dir.join(config::PROVIDERS_FILE);
        let store = ProviderStore::load().expect("an empty store");
        (store, path, guard)
    }

    fn sample(id: &str, key: Option<&str>) -> ProviderInput {
        ProviderInput {
            id: id.to_owned(),
            name: format!("Provider {id}"),
            base_url: format!("https://{id}.example/v1"),
            api_key: key.map(ToOwned::to_owned),
            models: vec![
                Model {
                    id: "small".into(),
                    name: Some("Small".into()),
                    modalities: vec![],
                    context_window: None,
                },
                Model {
                    id: "large".into(),
                    name: None,
                    modalities: vec![],
                    context_window: None,
                },
            ],
        }
    }

    #[test]
    fn a_missing_file_is_an_empty_configuration() {
        let (store, path, _env) = store();
        assert!(!path.exists(), "nothing was created by reading");
        assert_eq!(store.snapshot(), Providers::default());
        assert!(store.start_reference().is_none());
        assert!(store.session_config_options(None).is_none());
    }

    #[test]
    fn a_written_provider_reads_back() {
        let (store, path, _env) = store();
        store.upsert(sample("openai", Some("sk-test"))).unwrap();

        let reread = ProviderStore::with_path(&path).expect("a file to read");
        let providers = reread.snapshot();
        assert_eq!(providers.providers.len(), 1);
        assert_eq!(providers.providers[0].api_key, "sk-test");
        assert_eq!(providers.providers[0].base_url, "https://openai.example/v1");
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        let (store, path, _env) = store();
        store.upsert(sample("openai", Some("sk-test"))).unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "credentials are not world-readable");
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_loose_file_is_tightened() {
        let (store, path, _env) = store();
        std::fs::write(
            &path,
            r#"{"version":1,"providers":[{"id":"a","name":"A","baseUrl":"https://a.example/v1","apiKey":"k","models":[{"id":"m"}]}]}"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        store.upsert(sample("b", Some("k"))).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_absent_key_on_a_write_keeps_the_stored_one() {
        // The panel reads a masked key back. Saving it unchanged must not write
        // the mask in place of the credential.
        let (store, _path, _env) = store();
        store.upsert(sample("openai", Some("sk-secret"))).unwrap();

        let mut edit = sample("openai", None);
        edit.name = "Renamed".into();
        store.upsert(edit).unwrap();

        let providers = store.snapshot();
        assert_eq!(providers.providers[0].name, "Renamed");
        assert_eq!(providers.providers[0].api_key, "sk-secret");
    }

    #[test]
    fn an_empty_key_clears_it() {
        let (store, _path, _env) = store();
        store.upsert(sample("local", Some("sk-secret"))).unwrap();
        store.upsert(sample("local", Some(""))).unwrap();

        assert_eq!(store.snapshot().providers[0].api_key, "");
        assert_eq!(store.views()[0].api_key, None);
        assert!(!store.views()[0].has_key);
    }

    #[test]
    fn a_key_is_never_reported_whole() {
        let (store, _path, _env) = store();
        store
            .upsert(sample("openai", Some("sk-abcdefghijklmnop")))
            .unwrap();

        let view = &store.views()[0];
        assert_eq!(view.api_key.as_deref(), Some("sk-…mnop"));
        assert!(view.has_key);
    }

    #[test]
    fn a_short_key_shows_nothing_of_itself() {
        assert_eq!(mask("sk-1234"), Some("…".to_owned()));
        assert_eq!(mask(""), None);
    }

    #[test]
    fn a_provider_that_fails_validation_is_refused_and_nothing_is_written() {
        let (store, path, _env) = store();

        let mut bad = sample("OpenAI", None);
        bad.id = "OpenAI".into();
        assert!(matches!(store.upsert(bad), Err(ProviderError::Invalid(_))));

        let mut insecure = sample("openai", None);
        insecure.base_url = "ftp://openai.example/v1".into();
        assert!(matches!(
            store.upsert(insecure),
            Err(ProviderError::Invalid(_))
        ));

        let mut blank = sample("openai", None);
        blank.models = vec![Model {
            id: "  ".into(),
            name: None,
            modalities: vec![],
            context_window: None,
        }];
        assert!(matches!(
            store.upsert(blank),
            Err(ProviderError::Invalid(_))
        ));

        let mut duplicate = sample("openai", None);
        duplicate.models = vec![
            Model {
                id: "same".into(),
                name: None,
                modalities: vec![],
                context_window: None,
            },
            Model {
                id: "same".into(),
                name: None,
                modalities: vec![],
                context_window: None,
            },
        ];
        assert!(matches!(
            store.upsert(duplicate),
            Err(ProviderError::Invalid(_))
        ));

        assert!(!path.exists(), "a refused write leaves no file");
        assert!(store.snapshot().providers.is_empty());
    }

    #[test]
    fn a_slash_in_a_model_id_is_refused() {
        // It is the reference separator, so allowing it would make two different
        // references read as one.
        let (store, _path, _env) = store();
        let mut bad = sample("openai", None);
        bad.models[0].id = "a/b".into();
        assert!(matches!(store.upsert(bad), Err(ProviderError::Invalid(_))));
    }

    #[test]
    fn a_relative_base_url_is_refused() {
        assert!(valid_base_url("https://api.example/v1"));
        assert!(valid_base_url("http://127.0.0.1:8080/v1"));
        assert!(!valid_base_url("ftp://api.example/v1"));
        assert!(!valid_base_url("/v1"));
        assert!(!valid_base_url("api.example/v1"));
        assert!(!valid_base_url("https://api.example/v1/"));
        assert!(!valid_base_url("https://"));
        assert!(!valid_base_url("http://"));
        assert!(!valid_base_url("https:// /v1"));
    }

    #[test]
    fn a_reference_splits_on_the_first_slash_only() {
        assert_eq!(split_reference("openai/gpt-4o"), Some(("openai", "gpt-4o")));
        assert_eq!(split_reference("openai/a/b"), None);
        assert_eq!(split_reference("openai"), None);
        assert_eq!(split_reference("/gpt"), None);
        assert_eq!(split_reference("openai/"), None);
    }

    #[test]
    fn resolving_an_unknown_provider_or_model_says_which() {
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();

        assert!(matches!(
            store.resolve("missing/small"),
            Err(ProviderError::UnknownProvider(_))
        ));
        assert!(matches!(
            store.resolve("openai/missing"),
            Err(ProviderError::UnknownModel(_))
        ));
        assert!(matches!(
            store.resolve("nonsense"),
            Err(ProviderError::BadReference(_))
        ));
        let (provider, model) = store.resolve("openai/small").unwrap();
        assert_eq!(provider.id, "openai");
        assert_eq!(model.id, "small");
    }

    #[test]
    fn the_start_reference_is_the_default_when_it_resolves() {
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        store.upsert(sample("deepseek", None)).unwrap();
        store.set_default("deepseek/large").unwrap();

        assert_eq!(store.start_reference().as_deref(), Some("deepseek/large"));
    }

    #[test]
    fn without_a_default_there_is_nothing_to_start_on() {
        // Providers are not a choice: they are what there is to choose between.
        // Nothing is picked on the user's behalf, here or anywhere else.
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        store.upsert(sample("deepseek", None)).unwrap();

        assert_eq!(store.default_reference(), None);
        assert_eq!(store.start_reference(), None);
    }

    #[test]
    fn a_default_that_names_nothing_leaves_no_start_reference() {
        // Deleting the provider a default pointed into costs the default. What
        // is left is a store with a provider and nothing chosen, which is the
        // same state as one where nobody ever chose — not a licence to pick the
        // first thing in the file.
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        store.set_default("openai/small").unwrap();
        store.upsert(sample("deepseek", None)).unwrap();
        store.delete("openai").unwrap();

        assert_eq!(store.default_reference(), None, "the dangling default went");
        assert_eq!(store.start_reference(), None);
    }

    #[test]
    fn setting_a_default_that_names_nothing_is_refused() {
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        assert!(matches!(
            store.set_default("openai/missing"),
            Err(ProviderError::UnknownModel(_))
        ));
    }

    #[test]
    fn deleting_an_unknown_provider_is_refused() {
        let (store, _path, _env) = store();
        assert!(matches!(
            store.delete("missing"),
            Err(ProviderError::UnknownProvider(_))
        ));
    }

    #[test]
    fn deleting_keeps_the_other_providers_default() {
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        store.upsert(sample("deepseek", None)).unwrap();
        store.set_default("deepseek/large").unwrap();

        store.delete("openai").unwrap();

        assert_eq!(store.default_reference().as_deref(), Some("deepseek/large"));
    }

    #[test]
    fn the_selector_groups_by_provider() {
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        store.upsert(sample("deepseek", None)).unwrap();

        let options = store.session_config_options(None).expect("a selector");
        assert_eq!(options.len(), 1, "one option, the model");
        assert_eq!(options[0].id.0.as_ref(), MODEL_OPTION_ID);

        let srud_protocol::acp::SessionConfigKind::Select(select) = &options[0].kind else {
            panic!("the model option is a select");
        };
        let srud_protocol::acp::SessionConfigSelectOptions::Grouped(groups) = &select.options
        else {
            panic!("grouped by provider");
        };
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Provider openai");
        assert_eq!(groups[0].options.len(), 2);
        assert_eq!(groups[0].options[0].value.0.as_ref(), "openai/small");
        assert_eq!(groups[0].options[0].name, "Small");
        // A model with no name of its own is shown under its id.
        assert_eq!(groups[0].options[1].name, "large");
        assert_eq!(select.current_value.0.as_ref(), "");
    }

    #[test]
    fn a_selection_that_no_longer_resolves_is_still_shown() {
        // A session whose provider was deleted must read as unavailable rather
        // than silently as some other model.
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();

        let options = store.session_config_options(Some("gone/model")).unwrap();
        let srud_protocol::acp::SessionConfigKind::Select(select) = &options[0].kind else {
            panic!("select");
        };
        let srud_protocol::acp::SessionConfigSelectOptions::Grouped(groups) = &select.options
        else {
            panic!("grouped");
        };
        assert_eq!(select.current_value.0.as_ref(), "gone/model");
        assert_eq!(groups.len(), 2, "the unavailable one gets its own group");
        assert_eq!(groups[1].name, "Unavailable");
        assert_eq!(groups[1].options[0].value.0.as_ref(), "gone/model");
    }

    #[test]
    fn the_selector_opens_with_nothing_chosen() {
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();

        let options = store.session_config_options(None).unwrap();
        let srud_protocol::acp::SessionConfigKind::Select(select) = &options[0].kind else {
            panic!("select");
        };
        assert_eq!(
            select.current_value.0.as_ref(),
            "",
            "no model is chosen until someone chooses one"
        );
    }

    #[test]
    fn a_provider_with_no_models_yet_offers_no_selector() {
        // The provider is added first and its models after, so this is a state a
        // user passes through rather than one to refuse.
        let (store, _path, _env) = store();
        let mut bare = sample("openai", None);
        bare.models.clear();
        store.upsert(bare).unwrap();

        assert_eq!(store.session_config_options(None), None);
    }

    #[test]
    fn a_provider_only_offers_the_models_it_has() {
        // One provider with models and one without: the second is a group with
        // nothing in it, which is not a group.
        let (store, _path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();
        let mut bare = sample("deepseek", None);
        bare.models.clear();
        store.upsert(bare).unwrap();

        let options = store.session_config_options(None).expect("a selector");
        let srud_protocol::acp::SessionConfigKind::Select(select) = &options[0].kind else {
            panic!("select");
        };
        let srud_protocol::acp::SessionConfigSelectOptions::Grouped(groups) = &select.options
        else {
            panic!("grouped");
        };
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "Provider openai");
    }

    #[test]
    fn a_file_at_another_version_is_refused() {
        let (_store, path, _env) = store();
        std::fs::write(&path, r#"{"version":99,"providers":[]}"#).unwrap();

        assert!(matches!(
            ProviderStore::with_path(&path),
            Err(ProviderError::Parse { .. })
        ));
    }

    #[test]
    fn a_file_with_two_providers_of_one_id_is_refused() {
        let (_store, path, _env) = store();
        std::fs::write(
            &path,
            r#"{"version":1,"providers":[
                {"id":"a","name":"A","baseUrl":"https://a.example/v1","models":[{"id":"m"}]},
                {"id":"a","name":"B","baseUrl":"https://b.example/v1","models":[{"id":"n"}]}]}"#,
        )
        .unwrap();

        assert!(matches!(
            ProviderStore::with_path(&path),
            Err(ProviderError::Parse { .. })
        ));
    }

    #[test]
    fn a_file_that_is_not_json_is_refused() {
        let (_store, path, _env) = store();
        std::fs::write(&path, "not json at all").unwrap();

        assert!(matches!(
            ProviderStore::with_path(&path),
            Err(ProviderError::Parse { .. })
        ));
    }

    #[test]
    fn the_written_file_carries_the_format_version() {
        let (store, path, _env) = store();
        store.upsert(sample("openai", None)).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["version"], serde_json::json!(FORMAT_VERSION));
        assert_eq!(
            value["providers"][0]["baseUrl"],
            "https://openai.example/v1"
        );
        assert_eq!(
            value["providers"][0]["models"][0]["name"], "Small",
            "a named model keeps its name; an unnamed one omits the field"
        );
        assert!(value["providers"][0]["models"][1].get("name").is_none());
    }
}
