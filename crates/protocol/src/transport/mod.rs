//! Transport adapters.
//!
//! ACP is transport-agnostic: any channel that carries bidirectional
//! JSON-RPC messages is a legal transport, provided it preserves the message
//! format and lifecycle requirements. This module holds the payload types for
//! each transport SrudAgent supports.

pub mod stdio;
pub mod tauri;
