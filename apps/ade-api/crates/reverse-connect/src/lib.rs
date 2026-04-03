//! Reverse WebSocket connector with JSON-RPC control messages and multiplexed
//! `exec` and `pty` channels.

pub mod connector;
pub mod protocol;

pub use connector::{ConnectOptions, ConnectorError, DEFAULT_IDLE_TIMEOUT_SECONDS, connect};
