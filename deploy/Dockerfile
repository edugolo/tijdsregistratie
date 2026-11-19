# syntax=docker/dockerfile:1
#
# Build from the repository root:
#   docker build -f deploy/Dockerfile -t tijdsregistratie .

ARG RUST_VERSION=1.100
ARG DX_VERSION=0.7.10

# ---- builder ---------------------------------------------------------------
FROM rust:${RUST_VERSION}-trixie AS builder
ARG DX_VERSION

RUN rustup target add wasm32-unknown-unknown

# The Dioxus CLI changes rarely; keep it in its own cached layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    cargo install dioxus-cli --version ${DX_VERSION} --locked

WORKDIR /src
COPY . .

# Server binary, WASM client and public assets -> target/dx/app/release/web
# The admin CLI is built separately; both share the dependency cache.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    dx bundle --package app --release --platform web \
 && cargo build --release -p admin \
 && mkdir -p /out \
 && cp -r target/dx/app/release/web /out/web \
 && cp target/release/admin /out/admin

# ---- runtime ---------------------------------------------------------------
FROM debian:trixie-slim AS runtime

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && groupadd --system --gid 10001 tijd \
 && useradd --system --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin tijd

# The server finds its static files in a `public` directory next to the binary.
WORKDIR /app
COPY --from=builder /out/web/ /app/
COPY --from=builder /out/admin /usr/local/bin/admin

USER tijd
ENV IP=0.0.0.0 \
    PORT=8080
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/api/health | grep -q '"ok"' || exit 1

# Override the command to run the operator CLI, e.g. `docker compose run --rm app admin ...`.
CMD ["/app/server"]
