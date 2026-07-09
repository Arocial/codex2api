# codex2api

A local proxy that exposes the [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses) backed by your Codex subscription, so any Responses-API-compatible client can use Codex models without a separate API key.

Only the Responses API is exposed — there is no Chat Completions endpoint. Clients must speak the Responses API directly.

## How it works

`codex2api` runs a local HTTP server. Clients send standard Responses API requests to it; the proxy authenticates them with your stored Codex OAuth tokens and forwards them to `https://chatgpt.com/backend-api/codex/responses`. The SSE response stream is piped back verbatim — no buffering, no format conversion.

Authentication is shared with the Codex CLI: the same `~/.codex/auth.json` is used, and tokens are refreshed automatically.

## Requirements

- A current stable Rust toolchain
- A ChatGPT / OpenAI account with Codex access
- The Codex CLI available as `codex` (needed for `codex2api login`)

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

This delegates authentication to the installed `codex login` command, which
is configured to write credentials to the shared `~/.codex/auth.json` instead
of the OS keyring. If an existing `codex login` already created that file, this
step can be skipped. Set `CODEX2API_CODEX_BIN` if the Codex executable is not
named `codex`.

### 2. Start the proxy

```sh
CODEX2API_API_KEY='replace-with-a-long-random-secret' codex2api
```

The server listens on `127.0.0.1:3402` by default.
Clients must send this value as `Authorization: Bearer <key>` on `/v1/*`
requests. If the variable is omitted, a random ephemeral key is printed at
startup; setting the variable is recommended so the key remains stable.

### 3. Test the proxy

With the proxy running, send a streaming Responses API request:

```sh
curl -N http://127.0.0.1:3402/v1/responses \
  -H 'Authorization: Bearer replace-with-a-long-random-secret' \
  -H 'Content-Type: application/json' \
  -d '{
    "model": "gpt-5.5",
    "input": [
      {
        "role": "user",
        "content": [
          {
            "type": "input_text",
            "text": "Say hello in one short sentence."
          }
        ]
      }
    ]
  }'
```

### Options

```
Usage: codex2api [OPTIONS] [COMMAND]

Commands:
  login  Log in by delegating to the installed Codex CLI

Options:
      --listen <LISTEN>                      Local address to listen on [default: 127.0.0.1:3402]
      --codex-home <CODEX_HOME>              Codex home directory [default: ~/.codex]
      --backend-base-url <BACKEND_BASE_URL>  Backend base URL (env: CODEX2API_BACKEND_BASE_URL)
                                             [default: https://chatgpt.com/backend-api/codex]
      --client-version <CLIENT_VERSION>      Codex CLI version sent to the models endpoint
                                             (env: CODEX2API_CLIENT_VERSION)
      --api-key <API_KEY>                    Client API key (env: CODEX2API_API_KEY)
  -h, --help                                 Print help
```

`CODEX_HOME`, `CODEX2API_BACKEND_BASE_URL`, `CODEX2API_CLIENT_VERSION`, and
`CODEX2API_API_KEY` environment variables are respected. Override
`--backend-base-url` for FedRAMP, enterprise, or staging endpoints. The Docker
entrypoint sets `CODEX2API_CLIENT_VERSION` from `codex --version` unless it is
already set.

### Docker Compose

Compose requires a stable API key. Put it in a local `.env` file:

```dotenv
CODEX2API_API_KEY=replace-with-a-long-random-secret
```

Then start the service with
`docker compose -f docker/docker-compose.yml up`. The image includes the Codex
CLI. On the first start, the entrypoint detects that the persistent
`codex-home` volume has no `auth.json`, prints a login command, and waits. Run
that command in another terminal:

```sh
docker compose -f docker/docker-compose.yml exec codex2api \
  codex -c 'cli_auth_credentials_store="file"' login --device-auth
```

Open the displayed URL and enter the device code. Once authentication
completes, the waiting entrypoint detects the saved credentials and starts the
proxy automatically. Later starts reuse the saved credentials.

After the initial login, the service can be started in the background:

```sh
docker compose -f docker/docker-compose.yml up -d
```

The `.env` file is ignored by Git. To force a new login, remove
`/root/.codex/auth.json` from the `codex-home` volume and restart the service.

## API

### POST /v1/responses

Proxies to the Codex responses endpoint. Request and response formats follow the [OpenAI Responses API](https://platform.openai.com/docs/api-reference/responses) spec.

The maximum request body size is 32 MiB.

Two fields are managed automatically:

| Field | Behaviour |
|-------|-----------|
| `stream` | Always forced to `true`. Only SSE streaming is supported. |
| `store` | Defaults to `false` if not set by the client. An explicit client value is preserved. |

### GET /v1/models

Proxies to the Codex models endpoint and returns the available models. The
required backend `client_version` query parameter is added automatically.

```sh
curl http://127.0.0.1:3402/v1/models \
  -H 'Authorization: Bearer replace-with-a-long-random-secret'
```

## Logging

Log level is controlled by the `RUST_LOG` environment variable:

```sh
RUST_LOG=debug codex2api
```

## Project layout

```
src/
  main.rs    CLI entry point, delegated login, server startup
  state.rs   Shared application state (AuthManager, HTTP client)
  proxy.rs   Route handlers, body injection, SSE passthrough
```

## Notes

- Only the Responses API (`/v1/responses`, `/v1/models`) is proxied. Chat Completions and other OpenAI endpoints are not exposed.
- Only SSE streaming responses are supported. Non-streaming mode is not implemented.
- The HTTP client uses the Codex CLI `originator` and a compatible Codex
  User-Agent format.
- Token refresh is handled automatically. A 401 response triggers one refresh-and-retry cycle.
- Runtime authentication is implemented by the lightweight
  `crates/codex-auth-compat` crate. It preserves the Codex auth file format,
  refresh request, account/FedRAMP headers, originator, User-Agent format, and custom
  CA behavior without compiling the full Codex dependency graph.
- `scripts/check-codex-compat.sh /path/to/codex` checks whether the compatibility
  layer's pinned upstream revision needs review.
- Proxy-generated errors use the OpenAI-style `{ "error": { ... } }` JSON shape.

## Traffic fingerprint vs. the Codex CLI

The proxy sends the Codex CLI's `User-Agent` and `originator` headers, but is **not** byte-for-byte indistinguishable from a direct Codex CLI session. The CLI also attaches per-session identifiers (`session-id`, `thread-id`, `x-client-request-id`, `x-codex-installation-id`, `x-codex-window-id`, `OpenAI-Beta`, occasionally `x-oai-attestation`) and a fully-populated request body (`instructions`, `prompt_cache_key`, `service_tier`, …) that this proxy does not synthesize — it forwards whatever the client sends. A server-side observer can therefore tell the two apart if it looks closely. Perfect mimicry would additionally require a stable installation ID and device attestation, which is out of scope here.
