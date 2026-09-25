//! Functional agents (design 11): the manifest that is everything an agent
//! may do, the single-file package that carries it, and the WASM sandbox
//! that runs it with only the doors the core opens.
//!
//! This crate holds no data. Everything an agent reads or writes arrives
//! through [`Doors`], which the daemon implements against the store, the
//! egress gate and the actions; the host cannot reach any of them itself
//! because it does not link them.

pub mod manifest;
pub mod package;
mod runner;

pub use manifest::Manifest;
pub use package::Package;
pub use runner::{Doors, Invocation, Outcome, Run, RunError, Runner, wit};

/// What can go wrong before a run starts.
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// The manifest is malformed or asks for more than the core grants.
    #[error("manifest: {0}")]
    Manifest(String),
    /// The package is malformed.
    #[error("package: {0}")]
    Package(String),
    /// The sandbox could not be set up.
    #[error("engine: {0}")]
    Engine(String),
}
