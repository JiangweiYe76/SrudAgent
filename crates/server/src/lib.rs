//! The ACP v1 agent service for SrudAgent.
//!
//! This crate turns the core runtime into a protocol-compliant ACP agent:
//! it dispatches JSON-RPC requests, drives one [`run_turn`] per
//! `session/prompt`, and republishes the runtime's events as
//! `session/update` notifications.
//!
//! It is **transport-agnostic**: it listens on nothing. A host (the Tauri
//! shell, or a test) feeds it [`RpcRequest`] envelopes through
//! [`Agent::handle`] and receives the JSON-RPC reply, while
//! [`Agent::subscribe`] yields the outgoing notification stream.
//!
//! Behavioural boundaries:
//!
//! - In-memory sessions only — no rollout store, so `session/load`,
//!   `session/resume`, and the `_srud/unstable/*` extensions answer
//!   `METHOD_NOT_FOUND` and are not advertised in the capabilities.
//! - No approval flow — `session/request_permission` is not implemented;
//!   tool calls run unconditionally.
//! - Text-only prompts — non-text content blocks are rejected with
//!   `INVALID_PARAMS`, matching the declared prompt capabilities.
//!
//! What a host supplies and what it does not:
//!
//! - The model client — [`Agent::new`] takes one, so a test can hand over a
//!   scripted model and a live host can pick a provider.
//! - Nothing else. The tool set comes from [`standard_tools`], because the
//!   tools are read by the turn loop, which runs here rather than in the host.

mod agent;
pub mod config;
mod convert;
mod events;
mod sessions;
mod tools;

pub use agent::{Agent, RpcReply};
pub use sessions::SessionManager;
pub use tools::standard_tools;
