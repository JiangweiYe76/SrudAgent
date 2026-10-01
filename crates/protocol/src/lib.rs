//! The ACP v1 protocol contract for SrudAgent.
//!
//! This crate is the single source of type truth shared by every layer: the
//! server implements these types as an ACP agent, and clients consume them.
//! It carries no runtime behaviour — no dispatch, no connection management —
//! so it can sit at the bottom of the dependency graph.
//!
//! Standard ACP types are re-exported from the upstream
//! `agent-client-protocol-schema` crate rather than mirrored here, so the
//! contract cannot drift from the specification. Only SrudAgent's private
//! surface (the `_srud/*` methods and the `_meta.srud` payload) is defined
//! locally, in [`srud`].
//!
//! # Versioning
//!
//! The upstream schema crate is pinned to an exact version. An ACP wire
//! protocol version is selected by the schema release, not by the crate's own
//! semver — `1.x` of the crate implements wire v1. Bumping the pin is a
//! protocol review, not a dependency upgrade.

pub mod acp;
pub mod error;
pub mod srud;
pub mod transport;
