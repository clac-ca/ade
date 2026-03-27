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

FROM rust:1.94.0-alpine AS api-builder

ENV CARGO_TARGET_DIR=/cargo-target

WORKDIR /build

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates perl

COPY rust-toolchain.toml ./
COPY apps/ade-api/Cargo.toml apps/ade-api/Cargo.lock ./apps/ade-api/
COPY apps/ade-api/src ./apps/ade-api/src
COPY apps/ade-api/migrations ./apps/ade-api/migrations

RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=cargo-git-db,target=/usr/local/cargo/git/db \
    --mount=type=cache,id=ade-api-target,target=/cargo-target \
    cargo build --manifest-path apps/ade-api/Cargo.toml --locked --release --bin ade-api --bin ade-migrate && \
    install -Dm755 /cargo-target/release/ade-api /build/bin/ade-api && \
    install -Dm755 /cargo-target/release/ade-migrate /build/bin/ade-migrate

FROM alpine:3.22

WORKDIR /app

RUN apk add --no-cache ca-certificates \
    && addgroup -S ade \
    && adduser -S -D -H -G ade ade

ENV NODE_ENV=production

ARG BUILT_AT
ARG GIT_SHA
ARG SERVICE_VERSION

LABEL org.opencontainers.image.created=$BUILT_AT \
      org.opencontainers.image.revision=$GIT_SHA \
      org.opencontainers.image.version=$SERVICE_VERSION

COPY --from=web-builder --chown=ade:ade /build/apps/ade-web/dist ./public
COPY --from=api-builder --chown=ade:ade /build/bin/ade-api ./bin/ade-api
COPY --from=api-builder --chown=ade:ade /build/bin/ade-migrate ./bin/ade-migrate

USER ade:ade

EXPOSE 8000

CMD ["./bin/ade-api", "--host", "0.0.0.0", "--port", "8000"]
