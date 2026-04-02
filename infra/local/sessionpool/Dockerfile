# syntax=docker/dockerfile:1.7

FROM rust:1.94.1-bookworm AS builder

WORKDIR /workspace

COPY . .

RUN mkdir -p /workspace/bin

RUN --mount=type=cache,id=ade-sessionpool-target,target=/cargo-target \
    --mount=type=cache,id=ade-sessionpool-cargo-git,target=/usr/local/cargo/git/db \
    --mount=type=cache,id=ade-sessionpool-cargo-registry,target=/usr/local/cargo/registry \
    CARGO_TARGET_DIR=/cargo-target cargo build --locked --package session-pool-emulator --bin session-pool-emulator --release \
    && install -Dm755 /cargo-target/release/session-pool-emulator /workspace/bin/session-pool-emulator

FROM ubuntu:22.04

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        gzip \
        jq \
        perl \
        tar \
        wget \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /app /mnt/data /workspace

COPY --from=builder /workspace/bin/session-pool-emulator /usr/local/bin/session-pool-emulator

EXPOSE 9000

CMD ["session-pool-emulator", "serve", "--host", "0.0.0.0", "--port", "9000", "--workspace", "/workspace"]
