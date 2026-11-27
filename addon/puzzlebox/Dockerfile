# Build context is the repository root:
#   docker buildx build -f addon/puzzlebox/Dockerfile .
FROM node:24-alpine AS web
WORKDIR /src/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

# Pinned (Rust and cargo-chef versions) so the dependency layers below stay cached between
# releases; a floating tag like rust:1-alpine changes often and invalidates all of them.
FROM lukemathwalker/cargo-chef:0.1.78-rust-1.100.0-alpine3.22 AS chef
RUN apk add --no-cache musl-dev
WORKDIR /src

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates crates
RUN cargo chef prepare --recipe-path recipe.json

# Dependencies get their own layer, keyed only on the recipe (manifests + lockfile),
# so CI layer caches skip the long wasmtime/dependency build when only our code changes.
FROM chef AS rust
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --package puzzlebox-server --recipe-path recipe.json
COPY Cargo.toml Cargo.lock rustfmt.toml ./
COPY crates crates
COPY --from=web /src/frontend/dist frontend/dist
RUN cargo build --release --locked -p puzzlebox-server

FROM alpine:3.22
# tzdata: the Supervisor sets TZ, but chrono needs zoneinfo to resolve it (else local time = UTC).
RUN apk add --no-cache ca-certificates tini tzdata
COPY --from=rust /src/target/release/puzzlebox /usr/bin/puzzlebox
ENV PUZZLEBOX_DATA_DIR=/data
EXPOSE 8099
ENTRYPOINT ["/sbin/tini", "--", "/usr/bin/puzzlebox"]
LABEL io.hass.type="addon" io.hass.arch="amd64|aarch64"
