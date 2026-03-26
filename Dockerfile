FROM node:24.14.1-alpine AS web-builder

WORKDIR /build

RUN corepack enable

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml tsconfig.base.json ./
COPY apps/ade-web/package.json ./apps/ade-web/package.json
COPY apps/ade-api/package.json ./apps/ade-api/package.json

RUN pnpm install --frozen-lockfile

COPY apps/ade-web ./apps/ade-web

RUN pnpm --filter @ade/web build

FROM rust:1.94.0-alpine AS api-builder

WORKDIR /build

RUN apk add --no-cache build-base musl-dev pkgconfig ca-certificates

COPY rust-toolchain.toml ./
COPY apps/ade-api/Cargo.toml apps/ade-api/Cargo.lock ./apps/ade-api/
COPY apps/ade-api/src ./apps/ade-api/src
COPY apps/ade-api/migrations ./apps/ade-api/migrations

RUN cargo build --manifest-path apps/ade-api/Cargo.toml --locked --release --bin ade-api --bin ade-migrate

FROM alpine:3.22

WORKDIR /app

RUN apk add --no-cache ca-certificates

ENV NODE_ENV=production

ARG BUILT_AT
ARG GIT_SHA
ARG SERVICE_VERSION

LABEL org.opencontainers.image.created=$BUILT_AT \
      org.opencontainers.image.revision=$GIT_SHA \
      org.opencontainers.image.version=$SERVICE_VERSION

COPY --from=web-builder /build/apps/ade-web/dist ./public
COPY --from=api-builder /build/apps/ade-api/target/release/ade-api ./bin/ade-api
COPY --from=api-builder /build/apps/ade-api/target/release/ade-migrate ./bin/ade-migrate

EXPOSE 8000

CMD ["./bin/ade-api", "--host", "0.0.0.0", "--port", "8000"]
