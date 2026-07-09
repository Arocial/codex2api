# syntax=docker/dockerfile:1.7

FROM rust:1-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY src ./src

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked && \
    cp target/release/codex2api /usr/local/bin/codex2api

FROM node:22-bookworm-slim AS codex-cli

RUN npm install --global --omit=dev @openai/codex@latest && \
    npm cache clean --force

FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates libssl3 curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/codex2api /usr/local/bin/codex2api
COPY --from=codex-cli /usr/local/bin/node /usr/local/bin/node
COPY --from=codex-cli /usr/local/lib/node_modules /usr/local/lib/node_modules
COPY --chmod=755 docker/entrypoint.sh /usr/local/bin/entrypoint.sh

RUN ln -s ../lib/node_modules/@openai/codex/bin/codex.js /usr/local/bin/codex && \
    codex --version

ENV CODEX_HOME=/root/.codex
EXPOSE 3402

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["codex2api", "--listen", "0.0.0.0:3402"]
