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

FROM rust:1.94.0-alpine AS chef

WORKDIR /build

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates perl \
    && cargo install --locked cargo-chef

FROM chef AS planner

WORKDIR /build/apps/ade-api

COPY apps/ade-api ./

RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS api-builder

WORKDIR /build/apps/ade-api

ARG SERVICE_VERSION=0.1.0
ENV ADE_PLATFORM_VERSION="${SERVICE_VERSION}"

COPY --from=planner /build/apps/ade-api/recipe.json recipe.json

RUN cargo chef cook --release --recipe-path recipe.json

COPY apps/ade-api ./

RUN cargo build --locked --release --bin ade-api --bin ade-migrate \
    && install -Dm755 /build/apps/ade-api/target/release/ade-api /build/bin/ade-api \
    && install -Dm755 /build/apps/ade-api/target/release/ade-migrate /build/bin/ade-migrate

FROM python:3.12-slim AS python-builder

WORKDIR /build

RUN python -m pip install --no-cache-dir --upgrade pip build uv_build

COPY packages/ade-engine ./packages/ade-engine
COPY packages/ade-config ./packages/ade-config

RUN python -m build --wheel --outdir /dist /build/packages/ade-engine \
    && python -m build --wheel --outdir /dist /build/packages/ade-config

FROM alpine:3.22

WORKDIR /app

RUN apk add --no-cache ca-certificates \
    && addgroup -S ade \
    && adduser -S -D -H -G ade ade

ENV NODE_ENV=production
ENV ADE_ENGINE_WHEEL_PATH=/app/python/ade_engine.whl

COPY --from=web-builder --chown=ade:ade /build/apps/ade-web/dist ./public
COPY --from=api-builder --chown=ade:ade /build/bin/ade-api ./bin/ade-api
COPY --from=api-builder --chown=ade:ade /build/bin/ade-migrate ./bin/ade-migrate
COPY --from=python-builder --chown=ade:ade /dist/*.whl ./python/

RUN engine_wheel="$(basename ./python/ade_engine-*.whl)" \
    && config_wheel="$(basename ./python/ade_config-*.whl)" \
    && ln -sf "${engine_wheel}" ./python/ade_engine.whl \
    && ln -sf "${config_wheel}" ./python/ade_config.whl

USER ade:ade

EXPOSE 8000

CMD ["./bin/ade-api", "--host", "0.0.0.0", "--port", "8000"]
