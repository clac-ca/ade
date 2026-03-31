# reverse-connect

`reverse-connect` is a small CLI and Rust library for opening an outbound WebSocket connection and running remote `exec` and `pty` channels over JSON-RPC 2.0.

## How it works

1. The connector opens a WebSocket connection to a server.
2. It authenticates with a bearer token in the `Authorization` header.
3. It negotiates the `reverse-connect.v1` WebSocket subprotocol.
4. It sends `connector.hello`.
5. The server opens `exec` or `pty` channels with JSON-RPC requests.
6. The connector streams input, output, resize, signal, and exit events for each channel.

## CLI

```sh
reverse-connect connect \
  --url wss://example.com/connect \
  --bearer-token "$TOKEN"
```

Options:

- `--url`: WebSocket endpoint
- `--bearer-token`: bearer token sent in the `Authorization` header
- `--idle-timeout-seconds`: exit after this many idle seconds with no open channels

## Library

- [protocol.rs](./src/protocol.rs): JSON-RPC message and parameter types
- [connector.rs](./src/connector.rs): connection and channel runtime

