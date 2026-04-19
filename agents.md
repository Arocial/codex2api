# Agent guide

## Project

`codex2api` is a local HTTP proxy that exposes the OpenAI Responses API backed by a Codex subscription. It forwards `POST /v1/responses` and `GET /v1/models` to `https://chatgpt.com/backend-api/codex/` using OAuth tokens managed by the `codex-login` crate.

## Build

```sh
cargo build
cargo build --release
```

`codex-login` is a path dependency at `../codex/codex-rs/login`. The Codex source tree must be present at that relative path.

## Source layout

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI (clap), `login` subcommand, server startup |
| `src/state.rs` | `AppState`: `Arc<AuthManager>` + pre-configured `reqwest::Client` |
| `src/proxy.rs` | Route handlers, body injection, SSE passthrough, 401 retry |

## Key invariants

- **SSE only.** `stream: true` is always injected. Non-streaming mode is not supported and should not be added.
- **`store` default.** `store: false` is injected only when the client omits the field. An explicit client value must not be overwritten.
- **Auth headers.** Every backend request must carry `Authorization: Bearer <token>` and, when available, `ChatGPT-Account-ID`. The `reqwest::Client` from `build_reqwest_client()` already sets `User-Agent` and `originator`; do not replace it with a plain `reqwest::Client::new()`.
- **401 retry.** On a 401 response from the backend, call `auth_manager.refresh_token().await` once and retry. Do not retry more than once.

## Adding a new backend endpoint

1. Add a handler in `proxy.rs` following the pattern of `models_handler` (GET) or `responses_handler` (POST).
2. Register the route in `main.rs` inside `run_server`.
3. No new state fields are needed unless the endpoint requires distinct configuration.
