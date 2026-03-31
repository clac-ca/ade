use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";
pub const WEBSOCKET_SUBPROTOCOL: &str = "reverse-connect.v1";

pub const CONNECTOR_HELLO_METHOD: &str = "connector.hello";
pub const CHANNEL_OPEN_METHOD: &str = "channel.open";
pub const CHANNEL_STDIN_METHOD: &str = "channel.stdin";
pub const CHANNEL_RESIZE_METHOD: &str = "channel.resize";
pub const CHANNEL_SIGNAL_METHOD: &str = "channel.signal";
pub const CHANNEL_CLOSE_METHOD: &str = "channel.close";
pub const CHANNEL_DATA_METHOD: &str = "channel.data";
pub const CHANNEL_EXIT_METHOD: &str = "channel.exit";
pub const SESSION_ERROR_METHOD: &str = "session.error";
pub const SESSION_SHUTDOWN_METHOD: &str = "session.shutdown";

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ChannelId(pub String);

impl ChannelId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelKind {
    Exec,
    Pty,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelStream {
    Pty,
    Stderr,
    Stdout,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SignalName {
    Kill,
    Term,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorHelloParams {
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<HostInfo>,
    pub version: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EmptyResult {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelOpenParams {
    pub channel_id: ChannelId,
    pub command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    pub kind: ChannelKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pty: Option<PtySize>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelStdinParams {
    pub channel_id: ChannelId,
    pub data: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelResizeParams {
    pub channel_id: ChannelId,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSignalParams {
    pub channel_id: ChannelId,
    pub signal: SignalName,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelCloseParams {
    pub channel_id: ChannelId,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelDataParams {
    pub channel_id: ChannelId,
    pub data: String,
    pub stream: ChannelStream,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelExitParams {
    pub channel_id: ChannelId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionErrorParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<ChannelId>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum RpcMessage {
    Request(RequestMessage),
    Response(ResponseMessage),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RequestMessage {
    pub jsonrpc: String,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
}

impl RequestMessage {
    pub fn request(
        id: u64,
        method: impl Into<String>,
        params: impl Serialize,
    ) -> serde_json::Result<Self> {
        Ok(Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params: Some(serde_json::to_value(params)?),
            id: Some(id),
        })
    }

    pub fn notification(
        method: impl Into<String>,
        params: impl Serialize,
    ) -> serde_json::Result<Self> {
        Ok(Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params: Some(serde_json::to_value(params)?),
            id: None,
        })
    }

    pub fn parse_params<T>(&self) -> serde_json::Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        serde_json::from_value(self.params.clone().unwrap_or(Value::Null))
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResponseMessage {
    pub jsonrpc: String,
    pub id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ResponseError>,
}

impl ResponseMessage {
    pub fn success(id: u64, result: impl Serialize) -> serde_json::Result<Self> {
        Ok(Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub fn invalid_params(id: u64, message: impl Into<String>) -> Self {
        Self::error(id, -32602, message)
    }

    pub fn method_not_found(id: u64, method: impl Into<String>) -> Self {
        Self::error(
            id,
            -32601,
            format!("Method '{}' is not supported.", method.into()),
        )
    }

    pub fn internal_error(id: u64, message: impl Into<String>) -> Self {
        Self::error(id, -32603, message)
    }

    pub fn error(id: u64, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id,
            result: None,
            error: Some(ResponseError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ResponseError {
    pub code: i32,
    pub message: String,
}
