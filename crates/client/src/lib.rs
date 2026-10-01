//! Model provider adapters.
//!
//! Each module here implements [`srud_core::client::ModelClient`] on top of one
//! provider SDK, translating that SDK's streaming events into the core's
//! [`ModelEvent`](srud_core::client::ModelEvent) vocabulary. Protocol
//! differences stop at this boundary.

pub mod openai;
