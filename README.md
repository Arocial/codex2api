# codex2api

A local proxy that exposes the [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses) backed by your Codex subscription, so any Responses-API-compatible client can use Codex models without a separate API key.

Only the Responses API is exposed — there is no Chat Completions endpoint. Clients must speak the Responses API directly.

## How it works

`codex2api` runs a local HTTP server. Clients send standard Responses API requests to it; the proxy authenticates them with your stored Codex OAuth tokens and forwards them to `https://chatgpt.com/backend-api/codex/responses`. The SSE response stream is piped back verbatim — no buffering, no format conversion.

Authentication is shared with the Codex CLI: the same `~/.codex/auth.json` is used, and tokens are refreshed automatically.

## Requirements

- Rust toolchain (1.80+)
- A ChatGPT / OpenAI account with Codex access
- The [Codex CLI](https://github.com/openai/codex) source checked out alongside this repo at `../codex`

## Build

```sh
cargo build --release
```

The binary is at `target/release/codex2api`.

## Usage

### 1. Log in

```sh
codex2api login
```

This opens a browser window for the ChatGPT PKCE OAuth flow and writes credentials to `~/.codex/auth.json`. If you have already run `codex login`, this step can be skipped — the same credentials are used.

### 2. Start the proxy

```sh
codex2api
```

The server listens on `127.0.0.1:3402` by default.

### Options

```
Usage: codex2api [OPTIONS] [COMMAND]

Commands:
  login  Log in to ChatGPT / OpenAI using the browser-based PKCE flow

Options:
      --listen <LISTEN>                      Local address to listen on [default: 127.0.0.1:3402]
      --codex-home <CODEX_HOME>              Codex home directory [default: ~/.codex]
      --backend-base-url <BACKEND_BASE_URL>  Backend base URL (env: CODEX2API_BACKEND_BASE_URL)
                                             [default: https://chatgpt.com/backend-api/codex]
  -h, --help                                 Print help
```

`CODEX_HOME` and `CODEX2API_BACKEND_BASE_URL` environment variables are also respected. Override `--backend-base-url` for FedRAMP, enterprise, or staging endpoints.

## API

### POST /v1/responses

Proxies to the Codex responses endpoint. Request and response formats follow the [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses) spec.

Two fields are managed automatically:

| Field | Behaviour |
|-------|-----------|
| `stream` | Always forced to `true`. Only SSE streaming is supported. |
| `store` | Defaults to `false` if not set by the client. An explicit client value is preserved. |

### GET /v1/models

Proxies to the Codex models endpoint and returns the available models.

## Logging

Log level is controlled by the `RUST_LOG` environment variable:

```sh
RUST_LOG=debug codex2api
```

## Project layout

```
src/
  main.rs    CLI entry point, login flow, server startup
  state.rs   Shared application state (AuthManager, HTTP client)
  proxy.rs   Route handlers, body injection, SSE passthrough
```

## Notes

- Only the Responses API (`/v1/responses`, `/v1/models`) is proxied. Chat Completions and other OpenAI endpoints are not exposed.
- Only SSE streaming responses are supported. Non-streaming mode is not implemented.
- The HTTP client reuses the same User-Agent and `originator` headers as the Codex CLI (`codex_cli_rs`).
- Token refresh is handled automatically. A 401 response triggers one refresh-and-retry cycle.
- `codex-login` is referenced as a path dependency from the Codex source tree (`../codex/codex-rs/login`). The Codex source must be present at that relative path.

## Traffic fingerprint vs. the Codex CLI

The proxy sends the Codex CLI's `User-Agent` and `originator` headers, but is **not** byte-for-byte indistinguishable from a direct Codex CLI session. The CLI also attaches per-session identifiers (`session-id`, `thread-id`, `x-client-request-id`, `x-codex-installation-id`, `x-codex-window-id`, `OpenAI-Beta`, occasionally `x-oai-attestation`) and a fully-populated request body (`instructions`, `prompt_cache_key`, `service_tier`, …) that this proxy does not synthesize — it forwards whatever the client sends. A server-side observer can therefore tell the two apart if it looks closely. Perfect mimicry would additionally require a stable installation ID and device attestation, which is out of scope here.
