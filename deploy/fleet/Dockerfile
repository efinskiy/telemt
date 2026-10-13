# syntax=docker/dockerfile:1
FROM node:22-bookworm-slim AS frontend
WORKDIR /build
COPY admin/frontend/package.json admin/frontend/package-lock.json ./
RUN npm ci --no-audit --no-fund
COPY admin/frontend/ ./
RUN npm run build

FROM rust:1.88-bookworm AS admin-build
WORKDIR /build
COPY admin/backend/ ./
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && printf 'Cargo build jobs: %s\n' "$CARGO_BUILD_JOBS" \
    && cargo build --release --locked --jobs "$CARGO_BUILD_JOBS"

FROM rust:1.88-bookworm AS agent-build
WORKDIR /build
COPY agent/ ./
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && printf 'Cargo build jobs: %s\n' "$CARGO_BUILD_JOBS" \
    && cargo build --release --locked --jobs "$CARGO_BUILD_JOBS"

FROM rust:1.88-bookworm AS node-build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
COPY benches/ ./benches/
ARG CARGO_BUILD_JOBS
RUN export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(nproc)}" \
    && printf 'Cargo build jobs: %s\n' "$CARGO_BUILD_JOBS" \
    && cargo build --release --locked --jobs "$CARGO_BUILD_JOBS"

FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 telemt && useradd --uid 10001 --gid 10001 --no-create-home telemt
WORKDIR /app
USER 10001:10001

FROM runtime AS admin
COPY --from=admin-build /build/target/release/telemt-admin /app/telemt-admin
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
