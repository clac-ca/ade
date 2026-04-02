//! Sandbox-environment runtime kernel.
//!
//! ADE allocates a sandbox from the Azure session pool, prepares the shared
//! sandbox environment, installs the selected config, and then executes runs
//! inside that prepared environment.

mod assets;
mod manager;
mod provider;
mod rendezvous;

pub use ::reverse_connect::protocol::{
    ChannelId, ChannelKind, ChannelOpenParams, ChannelStream, PtySize, SignalName,
};
pub use manager::{SandboxEnvironment, SandboxEnvironmentEvent, SandboxEnvironmentManager};
