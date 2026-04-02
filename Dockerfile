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

COPY Cargo.lock ./Cargo.lock
COPY apps/ade-api ./apps/ade-api
COPY packages/reverse-connect ./packages/reverse-connect

ARG SERVICE_VERSION=0.1.0
ENV ADE_PLATFORM_VERSION="${SERVICE_VERSION}"

RUN --mount=type=cache,id=ade-target,target=/build/target \
    --mount=type=cache,id=ade-api-cargo-git,target=/usr/local/cargo/git/db \
    --mount=type=cache,id=ade-api-cargo-registry,target=/usr/local/cargo/registry \
    cargo build --manifest-path apps/ade-api/Cargo.toml --locked --release --bin ade-api --bin ade-migrate \
    && install -Dm755 /build/target/release/ade-api /build/bin/ade-api \
    && install -Dm755 /build/target/release/ade-migrate /build/bin/ade-migrate

FROM alpine:3.23

WORKDIR /app

RUN apk add --no-cache ca-certificates \
    && addgroup -S ade \
    && adduser -S -D -H -G ade ade

ENV NODE_ENV=production
ENV ADE_SANDBOX_ENVIRONMENT_ARCHIVE_PATH=/app/runtime/sandbox-environment.tar.gz

COPY --from=web-builder --chown=ade:ade /build/apps/ade-web/dist ./public
COPY --from=api-builder --chown=ade:ade /build/bin/ade-api ./bin/ade-api
COPY --from=api-builder --chown=ade:ade /build/bin/ade-migrate ./bin/ade-migrate
COPY --chown=ade:ade .package/sandbox-environment.tar.gz ./runtime/sandbox-environment.tar.gz

USER ade:ade

EXPOSE 8000

CMD ["./bin/ade-api", "--host", "0.0.0.0", "--port", "8000"]
