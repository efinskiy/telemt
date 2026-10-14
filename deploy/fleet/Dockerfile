# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim AS frontend
WORKDIR /build
COPY admin/frontend/package.json admin/frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY admin/frontend/ ./
RUN npm run build

# Keep dependency artifacts in ordinary layers so the GitHub cache exports them.
FROM rust:1.88-bookworm AS chef
RUN cargo install cargo-chef --version 0.1.71 --locked
WORKDIR /build

FROM chef AS admin-plan
COPY managed/ /build/managed/
WORKDIR /build/admin/backend
COPY admin/backend/ ./
RUN cargo chef prepare --recipe-path /recipe.json --bin telemt-admin

FROM chef AS admin-build
COPY managed/ /build/managed/
WORKDIR /build/admin/backend
COPY --from=admin-plan /recipe.json /recipe.json
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && cargo chef cook --release --locked --bin telemt-admin --recipe-path /recipe.json
COPY admin/backend/ ./
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && printf 'Cargo build jobs: %s\n' "$CARGO_BUILD_JOBS" \
    && cargo build --release --locked --bin telemt-admin --jobs "$CARGO_BUILD_JOBS"

FROM chef AS agent-plan
COPY agent/ ./
RUN cargo chef prepare --recipe-path /recipe.json --bin telemt-probe

FROM chef AS agent-build
COPY --from=agent-plan /recipe.json /recipe.json
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && cargo chef cook --release --locked --bin telemt-probe --recipe-path /recipe.json
COPY agent/ ./
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && printf 'Cargo build jobs: %s\n' "$CARGO_BUILD_JOBS" \
    && cargo build --release --locked --bin telemt-probe --jobs "$CARGO_BUILD_JOBS"

FROM chef AS node-plan
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY benches/ ./benches/
RUN cargo chef prepare --recipe-path /recipe.json --bin telemt

FROM chef AS node-build
COPY --from=node-plan /recipe.json /recipe.json
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && cargo chef cook --release --locked --bin telemt --recipe-path /recipe.json
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY benches/ ./benches/
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && printf 'Cargo build jobs: %s\n' "$CARGO_BUILD_JOBS" \
    && cargo build --release --locked --bin telemt --jobs "$CARGO_BUILD_JOBS"

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 telemt && useradd --uid 10001 --gid 10001 --no-create-home telemt
WORKDIR /app
USER 10001:10001

FROM runtime AS admin
COPY --from=node-build /build/target/release/telemt /app/telemt
COPY deploy/fleet/managed-public/ /app/public/
COPY --from=admin-build /build/admin/backend/target/release/telemt-admin /app/telemt-admin
COPY --from=frontend /build/dist /app/assets
ENV ADMIN_ASSETS_DIR=/app/assets ADMIN_LISTEN=0.0.0.0:8080
ENTRYPOINT ["/app/telemt-admin"]

FROM runtime AS probe
COPY --from=agent-build /build/target/release/telemt-probe /app/telemt-probe
ENTRYPOINT ["/app/telemt-probe"]

FROM runtime AS node
COPY --from=node-build /build/target/release/telemt /app/telemt
ENTRYPOINT ["/app/telemt"]
CMD ["/etc/telemt/config.toml"]

FROM chef AS managed-plan
COPY managed/ /build/managed/
WORKDIR /build/node-agent
COPY node-agent/ ./
RUN cargo chef prepare --recipe-path /recipe.json --bin telemt-node-agent

FROM chef AS managed-build
COPY managed/ /build/managed/
WORKDIR /build/node-agent
COPY --from=managed-plan /recipe.json /recipe.json
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && cargo chef cook --release --locked --bin telemt-node-agent --recipe-path /recipe.json
COPY node-agent/ ./
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && cargo build --release --locked --bin telemt-node-agent --jobs "$CARGO_BUILD_JOBS"

FROM nginx:1.28-bookworm AS managed-node
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates procps \
    && rm -rf /var/lib/apt/lists/*
COPY --from=node-build /build/target/release/telemt /app/telemt
COPY --from=managed-build /build/node-agent/target/release/telemt-node-agent /app/telemt-node-agent
COPY deploy/fleet/managed-public/ /app/public/
WORKDIR /var/lib/telemt-managed
STOPSIGNAL SIGTERM
ENTRYPOINT ["/app/telemt-node-agent"]
CMD []

# Explicit verification target; normal node and Fleet images never execute Rust tests.
FROM agent-build AS probe-tests

FROM node-build AS node-tests
COPY --from=frontend /usr/local/bin/node /usr/local/bin/node
