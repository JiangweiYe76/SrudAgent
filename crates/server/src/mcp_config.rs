//! MCP server configuration.
//!
//! One file holds every MCP server the user has configured: what to start, or
//! where to dial, and what it needs to run. It lives under the configuration
//! directory as `mcp.json`, beside the provider file and for the same reason —
//! it is the user's to edit.
//!
//! # What the file looks like
//!
//! No transport tag: a server with a `command` is started, one with a `url` is
//! dialed, and the shortest entry that could work is the one that is written.
//! `env` and `headers` are objects, because a hand-written mapping reads better
//! than a list of name/value rows.
//!
//! ```json
//! {
//!   "version": 1,
//!   "servers": [
//!     { "name": "docs", "command": "npx", "args": ["-y", "docs-mcp"], "env": { "TOKEN": "…" } },
//!     { "name": "remote", "url": "https://example.test/mcp", "headers": { "Authorization": "Bearer …" } }
//!   ]
//! }
//! ```
//!
//! The version may be left out — the file is hand-written often enough that
//! demanding it would only produce a parse error over a file that says what it
//! means — but a version this build does not know is refused rather than
//! guessed at.
//!
//! # What the file is not
//!
//! It is not what a session runs with. ACP carries that in `session/new` and
//! `session/load`: the client names the servers, the agent starts them. This
//! file is where a client keeps the list between sessions, which is why the
//! `_srud/unstable/mcp/*` methods exist, and why a server added here reaches a
//! session only once that session is created or re-loaded.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use srud_protocol::srud::methods::{McpEntry, McpServerSpec};

use crate::config;

/// The format this build reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// Everything the MCP server file holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMcpServers {
    /// The configured servers, in the order the file lists them.
    #[serde(default)]
    pub servers: Vec<StoredMcpServer>,
}

/// One server as the file describes it.
///
/// The fields are optional because which ones are present is what names the
/// transport. Unknown fields are refused rather than ignored: a hand-written
/// `"arge"` would otherwise start a server without the argument it was meant to
/// get, and say nothing about it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredMcpServer {
    /// The name this server is stored under, and the prefix of its tool names.
    pub name: String,
    /// The program to start. Set on a server the agent launches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Command-line arguments for [`StoredMcpServer::command`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Variables set for the process [`StoredMcpServer::command`] starts.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// The endpoint to dial. Set on a server reached over HTTP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Headers sent to [`StoredMcpServer::url`] with every request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
}

/// Why a server, or a change to one, was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InvalidMcpServer {
    /// The name is empty.
    #[error("An MCP server needs a name")]
    BlankName,
    /// The name carries characters a tool name cannot.
    #[error("`{0}` is not a name a tool can carry: letters, digits, `-` and `_` only")]
    BadName(String),
    /// The server names neither a command nor a URL.
    #[error("`{0}` needs either a command to start or a URL to dial")]
    NoTransport(String),
    /// The server names both.
    #[error("`{0}` has both a command and a URL; keep one of them")]
    TwoTransports(String),
    /// The command is blank.
    #[error("An MCP server needs a command to start")]
    BlankCommand,
    /// The URL is not one this build dials.
    #[error("`{0}` is not an HTTP or HTTPS URL")]
    BadUrl(String),
    /// An environment variable or header has no name.
    #[error("An MCP server has a variable or header with no name")]
    BlankEntryName,
    /// A variable or header is listed twice.
    #[error("`{0}` is listed twice")]
    DuplicateEntry(String),
    /// Two servers share a name.
    #[error("Two MCP servers are called `{0}`")]
    DuplicateName(String),
}

impl InvalidMcpServer {
    /// The rule this refusal names, as a token a client can word for itself.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::BlankName => "blank-name",
            Self::BadName(_) => "bad-name",
            Self::NoTransport(_) => "no-transport",
            Self::TwoTransports(_) => "two-transports",
            Self::BlankCommand => "blank-command",
            Self::BadUrl(_) => "bad-url",
            Self::BlankEntryName => "blank-entry-name",
            Self::DuplicateEntry(_) => "duplicate-entry",
            Self::DuplicateName(_) => "duplicate-name",
        }
    }

    /// The values the token's wording has to place, empty when it needs none.
    #[must_use]
    pub fn params(&self) -> serde_json::Value {
        match self {
            Self::BlankName | Self::BlankCommand | Self::BlankEntryName => serde_json::json!({}),
            Self::BadName(name) | Self::NoTransport(name) | Self::TwoTransports(name) => {
                serde_json::json!({ "name": name })
            }
            Self::BadUrl(url) => serde_json::json!({ "url": url }),
            Self::DuplicateEntry(name) | Self::DuplicateName(name) => {
                serde_json::json!({ "name": name })
            }
        }
    }
}

/// Why the MCP server file, or a change to it, was refused.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    /// There is no configuration directory to put the file in.
    #[error(transparent)]
    Config(#[from] config::ConfigError),

    /// The file could not be read or written.
    #[error("Cannot use {path}: {source}")]
    Io {
        /// The file that could not be used.
        path: PathBuf,
        /// What the filesystem said.
        source: std::io::Error,
    },

    /// The file does not hold what this build understands.
    #[error("{path} is not an MCP server file this build can read: {message}")]
    Parse {
        /// The file that could not be read.
        path: PathBuf,
        /// What was wrong with it.
        message: String,
    },

    /// A server was rejected, and the rule it broke.
    #[error("Invalid MCP server: {0}")]
    Invalid(#[from] InvalidMcpServer),

    /// No configured server has that name.
    #[error("No MCP server called `{0}`")]
    UnknownServer(String),
}

impl McpError {
    /// The rule this refusal names, as a token a client can word for itself.
    ///
    /// `None` when the failure is not a rule of ours: a filesystem error or a
    /// parse failure repeats words from somewhere else, and no table of ours can
    /// improve on them.
    #[must_use]
    pub fn kind(&self) -> Option<&'static str> {
        match self {
            Self::Config(_) | Self::Io { .. } | Self::Parse { .. } => None,
            Self::Invalid(rule) => Some(rule.kind()),
            Self::UnknownServer(_) => Some("unknown-server"),
        }
    }

    /// The values the token's wording has to place, empty when it needs none.
    #[must_use]
    pub fn params(&self) -> serde_json::Value {
        match self {
            Self::Invalid(rule) => rule.params(),
            Self::UnknownServer(name) => serde_json::json!({ "name": name }),
            Self::Config(_) | Self::Io { .. } | Self::Parse { .. } => serde_json::json!({}),
        }
    }
}

impl StoredMcpServer {
    /// This server as a client reads it, once it is known to be usable.
    fn to_spec(&self) -> Result<McpServerSpec, InvalidMcpServer> {
        let name = self.name.trim().to_owned();
        match (&self.command, &self.url) {
            (Some(command), None) => Ok(McpServerSpec::Stdio {
                name,
                command: command.trim().to_owned(),
                args: self.args.clone(),
                env: entries(&self.env),
            }),
            (None, Some(url)) => Ok(McpServerSpec::Http {
                name,
                url: url.trim().to_owned(),
                headers: entries(&self.headers),
            }),
            (Some(_), Some(_)) => Err(InvalidMcpServer::TwoTransports(name)),
            (None, None) => Err(InvalidMcpServer::NoTransport(name)),
        }
    }

    /// The stored shape of a server a client sent.
    fn from_spec(server: &McpServerSpec) -> Self {
        match server {
            McpServerSpec::Stdio {
                name,
                command,
                args,
                env,
            } => Self {
                name: name.clone(),
                command: Some(command.clone()),
                args: args.clone(),
                env: mapping(env),
                ..Self::default()
            },
            McpServerSpec::Http { name, url, headers } => Self {
                name: name.clone(),
                url: Some(url.clone()),
                headers: mapping(headers),
                ..Self::default()
            },
        }
    }
}

/// What an import did, and what the file says afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpImport {
    /// Every server, in file order.
    pub servers: Vec<McpServerSpec>,
    /// The names the import added.
    pub added: Vec<String>,
    /// The names it replaced, as they were stored before.
    pub replaced: Vec<String>,
}

/// Read once and kept, so listing the servers does not touch the disk on every
/// session the client starts.
///
/// Writes go through the store, so the file and the copy cannot disagree about
/// what the user just configured. The lock is a plain `RwLock` and the file
/// operations are synchronous: this file is a few kilobytes and is written when
/// the user edits a server, not on any path a turn takes.
#[derive(Debug)]
pub struct McpStore {
    path: PathBuf,
    inner: RwLock<Vec<McpServerSpec>>,
}

impl McpStore {
    /// Reads the MCP server file, or starts empty when there is none.
    ///
    /// # Errors
    ///
    /// [`McpError::Config`] when there is no configuration directory to look
    /// in, [`McpError::Io`] when the file cannot be read, and [`McpError::Parse`]
    /// when it holds something this build cannot interpret — including two
    /// servers by the same name. A missing file is not an error: it means
    /// nothing is configured yet.
    pub fn load() -> Result<Self, McpError> {
        let path = config::mcp_path()?;
        Self::with_path(path)
    }

    /// Creates a store over a specific file, reading it now.
    ///
    /// For a caller that names the file itself rather than taking the default:
    /// a test, or a host with its own configuration source.
    ///
    /// # Errors
    ///
    /// As [`McpStore::load`].
    pub fn with_path(path: impl Into<PathBuf>) -> Result<Self, McpError> {
        let path = path.into();
        let servers = read(&path)?;
        Ok(Self {
            path,
            inner: RwLock::new(servers),
        })
    }

    /// An empty store that writes to `path` on its first edit.
    ///
    /// For a host that must run without a readable MCP server file: the app
    /// stays usable, and the file it could not read is replaced only once the
    /// user actually saves something.
    #[must_use]
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            inner: RwLock::new(Vec::new()),
        }
    }

    /// The file this store reads and writes.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every configured server, in file order.
    #[must_use]
    pub fn servers(&self) -> Vec<McpServerSpec> {
        self.inner.read().expect("mcp lock poisoned").clone()
    }

    /// Reads the file again, so an edit made outside this process is seen.
    ///
    /// A server list is asked for when a client opens its panel, which is
    /// exactly when someone who just edited the file by hand expects to see
    /// what they wrote — and a file that no longer parses is reported then,
    /// rather than going unnoticed until the next launch.
    ///
    /// # Errors
    ///
    /// As [`McpStore::load`]. Nothing is changed in memory when the file cannot
    /// be read.
    pub fn reload(&self) -> Result<Vec<McpServerSpec>, McpError> {
        let servers = read(&self.path)?;
        *self.inner.write().expect("mcp lock poisoned") = servers.clone();
        Ok(servers)
    }

    /// Adds a server, or replaces the one with the same name.
    ///
    /// The comparison ignores case: two servers whose names differ only in case
    /// would be told apart on the wire but not by a reader of the file, and the
    /// second one would look like an edit of the first.
    ///
    /// # Errors
    ///
    /// [`McpError::Invalid`] when the server does not pass validation, or
    /// [`McpError::Io`] when the change cannot be written — in which case nothing
    /// is applied, so what a caller reads back is what is on disk.
    pub fn upsert(&self, server: McpServerSpec) -> Result<Vec<McpServerSpec>, McpError> {
        validate(&server)?;
        let mut servers = self.inner.write().expect("mcp lock poisoned");

        let name = server.name().to_owned();
        let mut next = servers.clone();
        match next
            .iter()
            .position(|entry| entry.name().eq_ignore_ascii_case(&name))
        {
            Some(index) => next[index] = server,
            None => next.push(server),
        }

        write(&self.path, &next)?;
        *servers = next;
        Ok(servers.clone())
    }

    /// Adds several servers, replacing the ones whose names are already there.
    ///
    /// The whole batch is checked before anything is written, so a paste that
    /// names one thing this build refuses leaves the file exactly as it was.
    /// A name that is already configured counts as a replacement rather than a
    /// duplicate: that is what makes "paste this from the server's README" work
    /// on a machine where the server was configured once before.
    ///
    /// # Errors
    ///
    /// [`McpError::Invalid`] when any server in the batch does not pass
    /// validation or two of them share a name, and [`McpError::Io`] when the
    /// batch cannot be written — in which case nothing is applied.
    pub fn import(&self, servers: Vec<McpServerSpec>) -> Result<McpImport, McpError> {
        if servers.is_empty() {
            return Ok(McpImport {
                servers: self.servers(),
                added: Vec::new(),
                replaced: Vec::new(),
            });
        }

        let mut batch = HashSet::new();
        for server in &servers {
            validate(server)?;
            if !batch.insert(server.name().to_ascii_lowercase()) {
                return Err(McpError::Invalid(InvalidMcpServer::DuplicateName(
                    server.name().to_owned(),
                )));
            }
        }

        let mut current = self.inner.write().expect("mcp lock poisoned");
        let mut next = current.clone();
        let mut added = Vec::new();
        let mut replaced = Vec::new();
        for server in servers {
            let name = server.name().to_owned();
            match next
                .iter()
                .position(|entry| entry.name().eq_ignore_ascii_case(&name))
            {
                Some(index) => {
                    replaced.push(next[index].name().to_owned());
                    next[index] = server;
                }
                None => {
                    next.push(server);
                    added.push(name);
                }
            }
        }

        write(&self.path, &next)?;
        *current = next.clone();
        Ok(McpImport {
            servers: next,
            added,
            replaced,
        })
    }

    /// Removes a server.
    ///
    /// # Errors
    ///
    /// [`McpError::UnknownServer`] when no server has that name, and
    /// [`McpError::Io`] when the removal cannot be written — in which case
    /// nothing is applied.
    pub fn delete(&self, name: &str) -> Result<Vec<McpServerSpec>, McpError> {
        let mut servers = self.inner.write().expect("mcp lock poisoned");
        if !servers
            .iter()
            .any(|entry| entry.name().eq_ignore_ascii_case(name))
        {
            return Err(McpError::UnknownServer(name.to_owned()));
        }

        let mut next = servers.clone();
        next.retain(|entry| !entry.name().eq_ignore_ascii_case(name));

        write(&self.path, &next)?;
        *servers = next;
        Ok(servers.clone())
    }
}

/// What is wrong with a server a client sent, if anything.
fn validate(server: &McpServerSpec) -> Result<(), InvalidMcpServer> {
    let name = server.name();
    if name.trim().is_empty() {
        return Err(InvalidMcpServer::BlankName);
    }
    // The name is part of every tool name the server contributes, and the agent
    // replaces anything outside this set with `_`: a name that needed replacing
    // could collide with another server's.
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(InvalidMcpServer::BadName(name.to_owned()));
    }

    match server {
        McpServerSpec::Stdio { command, env, .. } => {
            if command.trim().is_empty() {
                return Err(InvalidMcpServer::BlankCommand);
            }
            check_entries(env)?;
        }
        McpServerSpec::Http { url, headers, .. } => {
            if !is_http_url(url) {
                return Err(InvalidMcpServer::BadUrl(url.clone()));
            }
            check_entries(headers)?;
        }
    }
    Ok(())
}

// A row with no name cannot be sent at all, and a row named twice would have one
// of its two values silently win — neither survives a trip through an object.
fn check_entries(entries: &[McpEntry]) -> Result<(), InvalidMcpServer> {
    let mut seen = HashSet::new();
    for entry in entries {
        let name = entry.name.trim();
        if name.is_empty() {
            return Err(InvalidMcpServer::BlankEntryName);
        }
        if !seen.insert(name.to_ascii_lowercase()) {
            return Err(InvalidMcpServer::DuplicateEntry(name.to_owned()));
        }
    }
    Ok(())
}

fn is_http_url(url: &str) -> bool {
    let url = url.trim();
    (url.starts_with("http://") || url.starts_with("https://"))
        && !url.contains(char::is_whitespace)
        && url.len() > "https://".len()
}

fn entries(map: &BTreeMap<String, String>) -> Vec<McpEntry> {
    map.iter()
        .map(|(name, value)| McpEntry {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

fn mapping(entries: &[McpEntry]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|entry| (entry.name.trim().to_owned(), entry.value.clone()))
        .collect()
}

/// Reads an MCP server file.
///
/// A file that is not there is an empty document: nothing is configured yet.
fn read(path: &Path) -> Result<Vec<McpServerSpec>, McpError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(McpError::Io {
                path: path.to_path_buf(),
                source,
            })
        }
    };

    let raw: serde_json::Value = serde_json::from_str(&text).map_err(|error| McpError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    // A version that is not there is the version this build writes: the file is
    // meant to be typed by hand, and demanding a line that carries no meaning
    // would only teach the user to copy it.
    if let Some(version) = raw.get("version") {
        if version.as_u64() != Some(u64::from(FORMAT_VERSION)) {
            return Err(McpError::Parse {
                path: path.to_path_buf(),
                message: format!("format version {version}, expected {FORMAT_VERSION}"),
            });
        }
    }

    let document: StoredMcpServers =
        serde_json::from_value(raw).map_err(|error| McpError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;

    let mut servers = Vec::with_capacity(document.servers.len());
    for server in &document.servers {
        let spec = server.to_spec().map_err(|error| McpError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        validate(&spec).map_err(|error| McpError::Parse {
            path: path.to_path_buf(),
            message: error.to_string(),
        })?;
        if servers
            .iter()
            .any(|entry: &McpServerSpec| entry.name().eq_ignore_ascii_case(spec.name()))
        {
            return Err(McpError::Parse {
                path: path.to_path_buf(),
                message: format!("two servers are called `{}`", spec.name()),
            });
        }
        servers.push(spec);
    }
    Ok(servers)
}

/// Writes an MCP server file, readable only by its owner.
///
/// The variables and headers in it may carry credentials, so it is written in
/// the mode a credential file belongs in and replaced by rename: a crash
/// mid-write leaves the previous file whole.
fn write(path: &Path, servers: &[McpServerSpec]) -> Result<(), McpError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| McpError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }

    let document = StoredMcpServers {
        servers: servers.iter().map(StoredMcpServer::from_spec).collect(),
    };
    let mut value = serde_json::to_value(&document).map_err(|error| McpError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    // The version is written from the constant rather than carried in the type:
    // what a file says it is can only be what this build writes.
    value["version"] = serde_json::Value::from(FORMAT_VERSION);
    let text = serde_json::to_string_pretty(&value).map_err(|error| McpError::Parse {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;

    let mut temp_name = path.as_os_str().to_os_string();
    temp_name.push(".tmp");
    let temp = PathBuf::from(&temp_name);

    let written = std::fs::write(&temp, &text)
        .map_err(|source| McpError::Io {
            path: temp.clone(),
            source,
        })
        .and_then(|()| {
            config::restrict(&temp).map_err(|source| McpError::Io {
                path: temp.clone(),
                source,
            })
        })
        .and_then(|()| {
            std::fs::rename(&temp, path).map_err(|source| McpError::Io {
                path: path.to_path_buf(),
                source,
            })
        });
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use crate::test_env::{self, Guard};

    /// A store over a file of its own, and the file's path.
    fn store() -> (McpStore, PathBuf, Guard) {
        let guard = Guard::take(&[config::HOME_VAR]);
        let dir = test_env::unique_dir("srud-mcp");
        std::env::set_var(config::HOME_VAR, &dir);
        let path = dir.join(config::MCP_FILE);
        let store = McpStore::load().expect("an empty store");
        (store, path, guard)
    }

    fn stdio(name: &str, command: &str) -> McpServerSpec {
        McpServerSpec::Stdio {
            name: name.to_owned(),
            command: command.to_owned(),
            args: vec![],
            env: vec![],
        }
    }

    #[test]
    fn a_file_that_is_not_there_holds_no_server() {
        let (store, path, _env) = store();

        assert!(store.servers().is_empty());
        assert!(!path.exists(), "reading does not create the file");
    }

    #[test]
    fn a_hand_written_file_is_read_as_the_server_it_describes() {
        let dir = test_env::unique_dir("srud-mcp-hand");
        let path = dir.join(config::MCP_FILE);
        // No `version`, no transport tag, `env` as an object: what someone would
        // write by hand after reading the documentation.
        fs::write(
            &path,
            r#"{
              "servers": [
                { "name": "docs", "command": "npx", "args": ["-y", "docs-mcp"],
                  "env": { "TOKEN": "t" } },
                { "name": "remote", "url": "https://example.test/mcp",
                  "headers": { "Authorization": "Bearer t" } }
              ]
            }"#,
        )
        .expect("a hand-written file");

        let store = McpStore::with_path(&path).expect("readable");
        assert_eq!(
            store.servers(),
            vec![
                McpServerSpec::Stdio {
                    name: "docs".into(),
                    command: "npx".into(),
                    args: vec!["-y".into(), "docs-mcp".into()],
                    env: vec![McpEntry {
                        name: "TOKEN".into(),
                        value: "t".into()
                    }],
                },
                McpServerSpec::Http {
                    name: "remote".into(),
                    url: "https://example.test/mcp".into(),
                    headers: vec![McpEntry {
                        name: "Authorization".into(),
                        value: "Bearer t".into()
                    }],
                },
            ]
        );
    }

    #[test]
    fn a_misspelled_field_is_refused_rather_than_ignored() {
        // Ignoring it would start the server without the argument it was meant
        // to get, and say nothing about it.
        let dir = test_env::unique_dir("srud-mcp-typo");
        let path = dir.join(config::MCP_FILE);
        fs::write(
            &path,
            r#"{ "servers": [{ "name": "docs", "command": "run", "arge": ["-y"] }] }"#,
        )
        .expect("file");

        let error = McpStore::with_path(&path).expect_err("refused");
        assert!(
            matches!(error, McpError::Parse { .. }),
            "a typo is a parse failure: {error:?}"
        );
        assert!(error.to_string().contains("arge"), "{error}");
    }

    #[test]
    fn a_name_a_tool_cannot_carry_is_refused() {
        let dir = test_env::unique_dir("srud-mcp-name");
        let path = dir.join(config::MCP_FILE);
        fs::write(
            &path,
            r#"{ "servers": [{ "name": "my server", "command": "run" }] }"#,
        )
        .expect("file");

        let error = McpStore::with_path(&path).expect_err("refused");
        assert!(error.to_string().contains("my server"), "{error}");
    }

    #[test]
    fn a_server_needs_exactly_one_transport() {
        let dir = test_env::unique_dir("srud-mcp-transport");
        let path = dir.join(config::MCP_FILE);
        fs::write(
            &path,
            r#"{ "servers": [{ "name": "both", "command": "run", "url": "https://example.test" }] }"#,
        )
        .expect("file");
        let error = McpStore::with_path(&path).expect_err("refused");
        assert!(error.to_string().contains("both"), "{error}");

        fs::write(&path, r#"{ "servers": [{ "name": "neither" }] }"#).expect("file");
        let error = McpStore::with_path(&path).expect_err("refused");
        assert!(error.to_string().contains("neither"), "{error}");
    }

    #[test]
    fn two_servers_by_the_same_name_are_refused() {
        let dir = test_env::unique_dir("srud-mcp-duplicate");
        let path = dir.join(config::MCP_FILE);
        fs::write(
            &path,
            r#"{ "servers": [{ "name": "docs", "command": "a" }, { "name": "docs", "command": "b" }] }"#,
        )
        .expect("file");

        let error = McpStore::with_path(&path).expect_err("refused");
        assert!(error.to_string().contains("docs"), "{error}");
    }

    #[test]
    fn a_version_this_build_does_not_know_is_refused() {
        let dir = test_env::unique_dir("srud-mcp-version");
        let path = dir.join(config::MCP_FILE);
        fs::write(&path, r#"{ "version": 2, "servers": [] }"#).expect("file");

        let error = McpStore::with_path(&path).expect_err("refused");
        assert!(error.to_string().contains("format version"), "{error}");
    }

    #[test]
    fn a_save_writes_the_shape_a_reader_would_write() {
        let (store, path, _env) = store();

        store
            .upsert(McpServerSpec::Stdio {
                name: "docs".into(),
                command: "run".into(),
                args: vec!["--root".into()],
                env: vec![McpEntry {
                    name: "TOKEN".into(),
                    value: "t".into(),
                }],
            })
            .expect("saved");

        let text = fs::read_to_string(&path).expect("the file was written");
        // The reader's shape, not the wire's: a command rather than a transport
        // tag, and an object rather than a list of rows.
        assert!(text.contains("\"command\": \"run\""), "{text}");
        assert!(text.contains("\"env\": {"), "{text}");
        assert!(!text.contains("transport"), "{text}");
        // And it reads back as what was saved.
        assert_eq!(
            McpStore::with_path(&path).expect("readable").servers(),
            store.servers()
        );
    }

    #[test]
    fn a_url_server_is_written_as_a_url_and_read_back() {
        let (store, path, _env) = store();
        store
            .upsert(McpServerSpec::Http {
                name: "remote".into(),
                url: "https://example.test/mcp".into(),
                headers: vec![McpEntry {
                    name: "Authorization".into(),
                    value: "Bearer t".into(),
                }],
            })
            .expect("saved");

        let text = fs::read_to_string(&path).expect("the file was written");
        assert!(
            text.contains("\"url\": \"https://example.test/mcp\""),
            "{text}"
        );
        assert!(!text.contains("\"command\""), "{text}");
        assert_eq!(
            McpStore::with_path(&path).expect("readable").servers(),
            store.servers()
        );
    }

    #[test]
    fn reading_again_picks_up_a_file_edited_by_hand() {
        let (store, path, _env) = store();
        store.upsert(stdio("docs", "first")).expect("saved");

        // The user edits the file while the agent runs.
        fs::write(
            &path,
            r#"{ "servers": [{ "name": "docs", "command": "hand-edited" }] }"#,
        )
        .expect("written by hand");
        assert_eq!(
            store.servers(),
            vec![stdio("docs", "first")],
            "not yet read"
        );

        let reread = store.reload().expect("readable");
        assert_eq!(reread, vec![stdio("docs", "hand-edited")]);
        assert_eq!(store.servers(), reread, "the store answers what it read");
    }

    #[test]
    fn saving_the_same_name_replaces_that_server() {
        let (store, _path, _env) = store();
        store.upsert(stdio("docs", "first")).expect("saved");
        store.upsert(stdio("Docs", "second")).expect("saved");

        let servers = store.servers();
        assert_eq!(servers.len(), 1, "one server, edited");
        assert_eq!(servers[0].name(), "Docs");
        match &servers[0] {
            McpServerSpec::Stdio { command, .. } => assert_eq!(command, "second"),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    #[test]
    fn a_server_that_does_not_pass_is_not_written() {
        let (store, path, _env) = store();

        let error = store
            .upsert(stdio("", "run"))
            .expect_err("a blank name is refused");
        assert_eq!(error.kind(), Some("blank-name"));
        assert!(!path.exists(), "a refused server leaves no file");
    }

    #[test]
    fn an_import_adds_what_is_new_and_replaces_what_is_not() {
        let (store, _path, _env) = store();
        store.upsert(stdio("docs", "first")).expect("saved");

        let imported = store
            .import(vec![stdio("docs", "second"), stdio("files", "run")])
            .expect("imported");

        assert_eq!(imported.added, vec!["files".to_owned()]);
        assert_eq!(imported.replaced, vec!["docs".to_owned()]);
        assert_eq!(
            imported.servers,
            vec![stdio("docs", "second"), stdio("files", "run")],
            "a replaced server keeps its place, a new one is appended"
        );
        assert_eq!(store.servers(), imported.servers);
    }

    #[test]
    fn an_import_that_is_refused_leaves_the_file_as_it_was() {
        let (store, path, _env) = store();
        store.upsert(stdio("docs", "first")).expect("saved");
        let before = fs::read_to_string(&path).expect("the file exists");

        // The second entry is what fails: the first one must not be written.
        let error = store
            .import(vec![stdio("files", "run"), stdio("my server", "run")])
            .expect_err("a name a tool cannot carry is refused");

        assert_eq!(error.kind(), Some("bad-name"));
        assert_eq!(
            fs::read_to_string(&path).expect("the file exists"),
            before,
            "nothing of a refused batch reached the file"
        );
        assert_eq!(store.servers(), vec![stdio("docs", "first")]);
    }

    #[test]
    fn two_servers_of_the_same_name_in_one_batch_are_refused() {
        let (store, path, _env) = store();

        let error = store
            .import(vec![stdio("docs", "a"), stdio("Docs", "b")])
            .expect_err("one of the two would win silently");

        assert_eq!(error.kind(), Some("duplicate-name"));
        assert!(!path.exists(), "a refused batch writes no file");
    }

    #[test]
    fn an_empty_import_writes_nothing() {
        let (store, path, _env) = store();

        let imported = store.import(Vec::new()).expect("nothing to do");

        assert!(imported.servers.is_empty());
        assert!(!path.exists(), "there was nothing to write");
    }

    #[test]
    fn removing_a_server_that_is_not_there_is_refused() {
        let (store, _path, _env) = store();

        let error = store.delete("docs").expect_err("nothing to remove");
        assert!(matches!(error, McpError::UnknownServer(_)));
        assert_eq!(error.kind(), Some("unknown-server"));
    }

    #[test]
    fn a_row_named_twice_is_refused() {
        let (store, _path, _env) = store();

        let error = store
            .upsert(McpServerSpec::Stdio {
                name: "docs".into(),
                command: "run".into(),
                args: vec![],
                env: vec![
                    McpEntry {
                        name: "TOKEN".into(),
                        value: "a".into(),
                    },
                    McpEntry {
                        name: "token".into(),
                        value: "b".into(),
                    },
                ],
            })
            .expect_err("refused");
        assert_eq!(error.kind(), Some("duplicate-entry"));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let (store, path, _env) = store();
        store.upsert(stdio("docs", "run")).expect("saved");

        let mode = fs::metadata(&path).expect("the file").permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "variables may carry credentials");
    }
}
