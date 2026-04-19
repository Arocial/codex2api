# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked && \
    cp target/release/codex2api /usr/local/bin/codex2api

FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/codex2api /usr/local/bin/codex2api

ENV CODEX_HOME=/root/.codex
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/codex2api"]
CMD ["--listen", "0.0.0.0:8080"]
