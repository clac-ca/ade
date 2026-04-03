# syntax=docker/dockerfile:1.7

FROM rust:1.94.1-bookworm AS builder

WORKDIR /workspace

COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock
COPY src ./src

RUN cargo build --locked --release --bin session-pool-emulator \
    && install -Dm755 /workspace/target/release/session-pool-emulator /workspace/bin/session-pool-emulator

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
