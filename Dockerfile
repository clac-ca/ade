# syntax=docker/dockerfile:1.7

ARG SANDBOX_PYTHON_VERSION=3.12.11

FROM node:24.14.1-alpine AS web-builder

ENV PNPM_HOME="/pnpm"
ENV PATH="$PNPM_HOME:$PATH"

WORKDIR /build

RUN corepack enable

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml tsconfig.base.json ./
COPY apps/ade-web/package.json ./apps/ade-web/package.json
COPY apps/ade-api/package.json ./apps/ade-api/package.json

RUN --mount=type=cache,id=pnpm-store,target=/pnpm/store \
    pnpm fetch --frozen-lockfile

RUN --mount=type=cache,id=pnpm-store,target=/pnpm/store \
    pnpm install --offline --frozen-lockfile

COPY apps/ade-web ./apps/ade-web

RUN pnpm --filter @ade/web build

FROM rust:1.94.1-alpine AS api-binaries-builder

WORKDIR /build

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates perl

COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock
COPY apps/ade-api ./apps/ade-api
COPY packages/reverse-connect ./packages/reverse-connect

ARG SERVICE_VERSION=0.1.0
ENV ADE_PLATFORM_VERSION="${SERVICE_VERSION}"

RUN --mount=type=cache,id=ade-rust-target,target=/build/target,sharing=locked \
    --mount=type=cache,id=ade-rust-cargo-git,target=/usr/local/cargo/git/db,sharing=locked \
    --mount=type=cache,id=ade-rust-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --locked --release -p ade-api --bin ade-api --bin ade-migrate \
    && install -Dm755 /build/target/release/ade-api /build/bin/ade-api \
    && install -Dm755 /build/target/release/ade-migrate /build/bin/ade-migrate

FROM --platform=linux/amd64 rust:1.94.1-alpine AS reverse-connect-builder

WORKDIR /build

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates perl

COPY Cargo.toml ./Cargo.toml
COPY Cargo.lock ./Cargo.lock
COPY apps/ade-api ./apps/ade-api
COPY packages/reverse-connect ./packages/reverse-connect

RUN --mount=type=cache,id=ade-rust-target,target=/build/target,sharing=locked \
    --mount=type=cache,id=ade-rust-cargo-git,target=/usr/local/cargo/git/db,sharing=locked \
    --mount=type=cache,id=ade-rust-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    cargo build --locked --release -p reverse-connect --bin reverse-connect \
    && install -Dm755 /build/target/release/reverse-connect /build/bin/reverse-connect

FROM --platform=linux/amd64 python:${SANDBOX_PYTHON_VERSION}-slim-bullseye AS sandbox-python

FROM python:${SANDBOX_PYTHON_VERSION}-slim-bullseye AS ade-engine-wheel-builder

WORKDIR /build

COPY packages/ade-engine ./packages/ade-engine

RUN mkdir -p /out \
    && python -m pip wheel --no-deps --wheel-dir /out /build/packages/ade-engine

FROM python:${SANDBOX_PYTHON_VERSION}-slim-bullseye AS sandbox-wheelhouse-builder

COPY --from=ade-engine-wheel-builder /out /wheel-src

RUN mkdir -p /out \
    && python -m pip download --dest /out /wheel-src/*.whl

FROM alpine:3.23 AS sandbox-environment-builder

WORKDIR /staging

COPY apps/ade-api/sandbox-environment/rootfs ./
COPY --from=reverse-connect-builder /build/bin/reverse-connect ./app/ade/bin/reverse-connect
COPY --from=sandbox-python /usr/local ./app/ade/python/current
COPY --from=sandbox-wheelhouse-builder /out ./app/ade/wheelhouse/base

RUN mkdir -p /out \
    && tar -C /staging -czf /out/sandbox-environment.tar.gz app

FROM scratch AS sandbox-environment-artifact

COPY --from=sandbox-environment-builder /out/sandbox-environment.tar.gz /sandbox-environment.tar.gz

FROM alpine:3.23

WORKDIR /app

RUN apk add --no-cache ca-certificates \
    && addgroup -S ade \
    && adduser -S -D -H -G ade ade

ENV NODE_ENV=production
ENV ADE_SANDBOX_ENVIRONMENT_ARCHIVE_PATH=/app/runtime/sandbox-environment.tar.gz

COPY --from=web-builder --chown=ade:ade /build/apps/ade-web/dist ./public
COPY --from=api-binaries-builder --chown=ade:ade /build/bin/ade-api ./bin/ade-api
COPY --from=api-binaries-builder --chown=ade:ade /build/bin/ade-migrate ./bin/ade-migrate
COPY --from=sandbox-environment-builder --chown=ade:ade /out/sandbox-environment.tar.gz ./runtime/sandbox-environment.tar.gz

USER ade:ade

EXPOSE 8000

CMD ["./bin/ade-api", "--host", "0.0.0.0", "--port", "8000"]
