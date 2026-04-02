# reverse-connect

`reverse-connect` is a small CLI and Rust library for opening an outbound WebSocket connection and running remote `exec` and `pty` channels over JSON-RPC 2.0.

## How it works

1. The connector opens a WebSocket connection to a server.
2. It authenticates with a bearer token in the `Authorization` header.
3. It sends `connector.hello`.
4. The server opens `exec` or `pty` channels with JSON-RPC requests.
5. The connector streams input, output, resize, signal, and exit events for each channel.

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

## Build Artifact

ADE builds the Linux `amd64` `reverse-connect` binary inside the root
platform Dockerfile and packages it into the sandbox-environment tarball.
It is not a runtime image for Azure session containers; production still uses
vanilla shell sessions and the API uploads the binary into them.
