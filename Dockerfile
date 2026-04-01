# syntax=docker/dockerfile:1.7

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

FROM rust:1.94.1-alpine AS api-builder

WORKDIR /build

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates perl

COPY Cargo.toml Cargo.lock ./
COPY apps/ade-api ./apps/ade-api
COPY infra/local/sessionpool ./infra/local/sessionpool
COPY packages/reverse-connect ./packages/reverse-connect

ARG SERVICE_VERSION=0.1.0
ENV ADE_PLATFORM_VERSION="${SERVICE_VERSION}"

RUN --mount=type=cache,id=ade-target,target=/build/target \
    --mount=type=cache,id=ade-api-cargo-git,target=/usr/local/cargo/git/db \
    --mount=type=cache,id=ade-api-cargo-registry,target=/usr/local/cargo/registry \
    cargo build --locked --release -p ade-api --bin ade-api --bin ade-migrate -p reverse-connect --bin reverse-connect \
    && install -Dm755 /build/target/release/ade-api /build/bin/ade-api \
    && install -Dm755 /build/target/release/ade-migrate /build/bin/ade-migrate \
    && install -Dm755 /build/target/release/reverse-connect /build/bin/reverse-connect

FROM python:3.12.11-slim-bullseye AS python-builder

WORKDIR /build

RUN python -m pip install --no-cache-dir --upgrade pip build uv_build

COPY packages/ade-engine ./packages/ade-engine
COPY packages/ade-config ./packages/ade-config

RUN python -m build --wheel --outdir /dist /build/packages/ade-engine \
    && python -m build --wheel --outdir /dist /build/packages/ade-config \
    && mkdir -p /wheelhouse/base \
    && python -m pip download --dest /wheelhouse/base /dist/ade_engine-*.whl \
    && tar -C /usr/local -czf /dist/python-3.12.11-linux-x86_64.tar.gz .

FROM alpine:3.23

WORKDIR /app

RUN apk add --no-cache ca-certificates \
    && addgroup -S ade \
    && adduser -S -D -H -G ade ade

ENV NODE_ENV=production

COPY --from=web-builder --chown=ade:ade /build/apps/ade-web/dist ./public
COPY --from=api-builder --chown=ade:ade /build/bin/ade-api ./bin/ade-api
COPY --from=api-builder --chown=ade:ade /build/bin/ade-migrate ./bin/ade-migrate
COPY --from=api-builder --chown=ade:ade /build/bin/reverse-connect ./.package/session-bundle/bin/reverse-connect
COPY --chown=ade:ade apps/ade-api/assets/session-bundle/bin/prepare.sh ./.package/session-bundle/bin/prepare.sh
COPY --chown=ade:ade apps/ade-api/assets/session-bundle/bin/run.py ./.package/session-bundle/bin/run.py
COPY --from=python-builder --chown=ade:ade /dist/python-3.12.11-linux-x86_64.tar.gz ./.package/session-bundle/python/
COPY --from=python-builder --chown=ade:ade /wheelhouse/base/*.whl ./.package/session-bundle/wheelhouse/base/
COPY --from=python-builder --chown=ade:ade /dist/ade_config-*.whl /tmp/session-configs/

RUN config_wheel="$(basename /tmp/session-configs/ade_config-*.whl)" \
    && mkdir -p ./.package/session-configs/workspace-a/config-v1 \
    && mkdir -p ./.package/session-configs/workspace-b/config-v2 \
    && cp "/tmp/session-configs/${config_wheel}" ./.package/session-configs/workspace-a/config-v1/ \
    && cp "/tmp/session-configs/${config_wheel}" ./.package/session-configs/workspace-b/config-v2/

USER ade:ade

EXPOSE 8000

CMD ["./bin/ade-api", "--host", "0.0.0.0", "--port", "8000"]
