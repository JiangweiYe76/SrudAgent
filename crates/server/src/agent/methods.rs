//! The method table an agent dispatches requests through.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde_json::Value;
use srud_protocol::acp::methods::{
    INITIALIZE, SESSION_CANCEL, SESSION_CLOSE, SESSION_DELETE, SESSION_LIST, SESSION_LOAD,
    SESSION_NEW, SESSION_PROMPT, SESSION_RESUME, SESSION_SET_CONFIG_OPTION, SESSION_SET_MODE,
    SRUD_MCP_DELETE, SRUD_MCP_IMPORT, SRUD_MCP_LIST, SRUD_MCP_SET, SRUD_MCP_TOOLS,
    SRUD_PROVIDER_DELETE, SRUD_PROVIDER_LIST, SRUD_PROVIDER_SET, SRUD_PROVIDER_SET_DEFAULT,
    SRUD_SESSION_FORK, SRUD_SESSION_REGENERATE, SRUD_SESSION_SET_TITLE, SRUD_SESSION_STEER,
    SRUD_SKILLS_LIST, SRUD_SKILLS_READ,
};
use srud_protocol::acp::{AcpError, CloseSessionResponse, DeleteSessionResponse};

use super::{serialize, Agent};

/// The reply one method produces once its work has settled.
pub type MethodFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, AcpError>> + Send + 'a>>;

/// One method's implementation over an [`Agent`].
pub type MethodHandler = for<'a> fn(&'a Agent, Value) -> MethodFuture<'a>;

// Registers one built-in method as a function with the common signature.
macro_rules! register {
    ($table:expr, $name:ident, $method:expr, |$agent:ident, $params:ident| $body:expr) => {{
        fn $name<'a>($agent: &'a Agent, $params: Value) -> MethodFuture<'a> {
            Box::pin(async move { $body })
        }
        $table.insert($method, $name);
    }};
}

/// The methods an agent answers, keyed by their wire names.
pub struct MethodTable {
    handlers: HashMap<String, MethodHandler>,
}

impl MethodTable {
    /// Creates a table with no methods.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }

    /// Creates a table with every method this build implements.
    #[must_use]
    pub fn builtin() -> Self {
        let mut table = Self::empty();
        register!(table, initialize, INITIALIZE, |agent, params| agent
            .handle_initialize(params)
            .map(serialize));
        register!(table, new_session, SESSION_NEW, |agent, params| agent
            .handle_new_session(params)
            .await
            .map(serialize));
        register!(table, load, SESSION_LOAD, |agent, params| agent
            .handle_load(params)
            .await
            .map(serialize));
        register!(table, resume, SESSION_RESUME, |agent, params| agent
            .handle_resume(params)
            .await
            .map(serialize));
        register!(table, prompt, SESSION_PROMPT, |agent, params| agent
            .handle_prompt(params)
            .await
            .map(serialize));
        register!(table, cancel, SESSION_CANCEL, |agent, params| agent
            .handle_cancel(params)
            .map(|()| Value::Object(Default::default())));
        register!(table, list, SESSION_LIST, |agent, params| agent
            .handle_list(params)
            .await
            .map(serialize));
        register!(table, close, SESSION_CLOSE, |agent, params| agent
            .handle_close(params)
            .await
            .map(|()| serialize(CloseSessionResponse::new())));
        register!(table, delete, SESSION_DELETE, |agent, params| agent
            .handle_delete(params)
            .await
            .map(|()| serialize(DeleteSessionResponse::new())));
        register!(
            table,
            set_config_option,
            SESSION_SET_CONFIG_OPTION,
            |agent, params| agent.handle_set_config_option(params).await.map(serialize)
        );
        register!(table, set_mode, SESSION_SET_MODE, |agent, params| agent
            .handle_set_mode(params)
            .await
            .map(serialize));
        register!(table, set_title, SRUD_SESSION_SET_TITLE, |agent, params| {
            agent.handle_set_title(params).await.map(serialize)
        });
        register!(table, fork, SRUD_SESSION_FORK, |agent, params| agent
            .handle_fork(params)
            .await
            .map(serialize));
        register!(table, steer, SRUD_SESSION_STEER, |agent, params| agent
            .handle_steer(params)
            .await
            .map(serialize));
        register!(
            table,
            regenerate,
            SRUD_SESSION_REGENERATE,
            |agent, params| agent.handle_regenerate(params).await.map(serialize)
        );
        register!(table, skills_list, SRUD_SKILLS_LIST, |agent, params| agent
            .handle_skills_list(params));
        register!(table, skill_read, SRUD_SKILLS_READ, |agent, params| agent
            .handle_skill_read(params));
        register!(table, provider_list, SRUD_PROVIDER_LIST, |agent, params| {
            agent.handle_provider_list(params)
        });
        register!(table, provider_set, SRUD_PROVIDER_SET, |agent, params| {
            agent.handle_provider_set(params)
        });
        register!(
            table,
            provider_delete,
            SRUD_PROVIDER_DELETE,
            |agent, params| agent.handle_provider_delete(params)
        );
        register!(
            table,
            provider_set_default,
            SRUD_PROVIDER_SET_DEFAULT,
            |agent, params| agent.handle_provider_set_default(params)
        );
        register!(table, mcp_list, SRUD_MCP_LIST, |agent, params| agent
            .handle_mcp_list(params));
        register!(table, mcp_set, SRUD_MCP_SET, |agent, params| agent
            .handle_mcp_set(params));
        register!(table, mcp_delete, SRUD_MCP_DELETE, |agent, params| agent
            .handle_mcp_delete(params));
        register!(table, mcp_import, SRUD_MCP_IMPORT, |agent, params| agent
            .handle_mcp_import(params));
        register!(table, mcp_tools, SRUD_MCP_TOOLS, |agent, params| agent
            .handle_mcp_tools(params)
            .await);
        table
    }

    /// Adds or replaces the implementation of one method.
    pub fn insert(&mut self, method: impl Into<String>, handler: MethodHandler) {
        self.handlers.insert(method.into(), handler);
    }

    /// Looks up the implementation of one method.
    #[must_use]
    pub fn get(&self, method: &str) -> Option<&MethodHandler> {
        self.handlers.get(method)
    }
}

impl Default for MethodTable {
    fn default() -> Self {
        Self::builtin()
    }
}
