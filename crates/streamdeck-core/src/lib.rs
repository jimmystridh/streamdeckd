//! Hardware- and network-free domain model for `streamdeckd`.
//!
//! This crate owns configuration, persistent state, the Pomodoro state machine,
//! press/navigation state machines, the deadline scheduler, integration payload
//! parsers, and the semantic key-view model. It deliberately depends on no HID
//! library, no macOS framework, and no HTTP client so every rule in it can be
//! tested without hardware or network access.

pub mod cache;
pub mod config;
pub mod control;
pub mod deadline;
pub mod integrations;
pub mod model;
pub mod nav;
pub mod pages;
pub mod pomodoro;
pub mod press;
pub mod snapshot;
pub mod state;
pub mod text;
pub mod view;

pub use model::{Grid, KeyPosition, PageId};
