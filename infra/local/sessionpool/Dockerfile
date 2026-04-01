FROM rust:1.94.1-bookworm AS builder

WORKDIR /workspace

COPY . .

RUN cargo build --locked --package session-pool-emulator --bin session-pool-emulator --release

FROM ubuntu:22.04

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install --yes --no-install-recommends \
        bash \
        ca-certificates \
        curl \
        gzip \
        jq \
        perl \
        python3 \
        tar \
        wget \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /app /mnt/data /workspace

COPY --from=builder /workspace/target/release/session-pool-emulator /usr/local/bin/session-pool-emulator

EXPOSE 9000

CMD ["session-pool-emulator", "serve", "--host", "0.0.0.0", "--port", "9000", "--workspace", "/workspace"]
