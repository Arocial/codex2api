# Agent guide

## Project

`codex2api` is a local HTTP proxy that exposes the OpenAI Responses API backed by a Codex subscription. It forwards `POST /v1/responses` and `GET /v1/models` to `https://chatgpt.com/backend-api/codex/` using OAuth tokens shared with the Codex CLI.

## Build

```sh
cargo build
cargo build --release
```

The lightweight `crates/codex-auth-compat` crate implements file auth and token
refresh against a pinned Codex revision. Login delegates to the installed
`codex login` command.

## Source layout

| File | Purpose |
|------|---------|
| `src/main.rs` | CLI (clap), `login` subcommand, server startup |
| `src/state.rs` | `AppState`: `Arc<AuthManager>` + separate pre-configured OAuth/backend `reqwest::Client`s |
| `src/proxy.rs` | Route handlers, body injection, SSE passthrough, 401 retry |
| `crates/codex-auth-compat` | Codex-compatible auth file, JWT, refresh, and HTTP defaults |
| `scripts/check-codex-compat.sh` | Detect upstream compatibility review requirements |

## Key invariants

- **SSE only.** `stream: true` is always injected. Non-streaming mode is not supported and should not be added.
- **`store` default.** `store: false` is injected only when the client omits the field. An explicit client value must not be overwritten.
- **Auth headers.** Every backend request must carry `Authorization: Bearer <token>` and, when available, `ChatGPT-Account-ID`. The backend `reqwest::Client` from `build_reqwest_client_with_cookie_store()` already sets `User-Agent` and `originator`; do not replace it with a plain `reqwest::Client::new()`.
- **401 retry.** On a 401 response from the backend, call `auth_manager.refresh_token().await` once and retry. Do not retry more than once.
- **Backend cookies.** The backend-only `reqwest::Client` uses a process-local shared `cookie_store` with standard domain/path/secure/expiry handling. An account change rebuilds that client and clears the session/turn ID mappings so account-bound state is not reused. Keep OAuth refresh on its separate cookie-free client, and never forward `Set-Cookie` to downstream clients.
- **Request limit.** Responses request bodies are limited to 32 MiB because
  they are parsed and serialized in memory.
- **Session identity.** `X-Session-Id` takes precedence over `session-id`.
  Session, thread, window, cache, and turn metadata must be derived from one
  request context so header and body projections remain identical.
- **Installation identity.** Reuse `$CODEX_HOME/installation_id`; never create
  a new installation ID per request or process restart.
- **Error format.** Proxy-generated API errors use the OpenAI-style
  `{ "error": { ... } }` JSON shape.
- **HTTP logs.** The global trace/request-ID layers log method, URI, version,
  status, and latency without request bodies or authorization headers. Keep
  handler errors inside that request span so details correlate through
  `X-Request-Id`.

## Adding a new backend endpoint

1. Add a handler in `proxy.rs` following the pattern of `models_handler` (GET) or `responses_handler` (POST).
2. Register the route in `main.rs` inside `run_server`.
3. No new state fields are needed unless the endpoint requires distinct configuration.
