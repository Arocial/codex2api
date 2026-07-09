#!/usr/bin/env bash
set -euo pipefail

expected_rev="996aa23e4ce900468047ed3ec57d1e7271f8d6de"
repo="${1:-../codex}"

if ! git -C "$repo" rev-parse --git-dir >/dev/null 2>&1; then
  echo "usage: $0 /path/to/openai-codex" >&2
  exit 2
fi

actual_rev="$(git -C "$repo" rev-parse HEAD)"
if [[ "$actual_rev" != "$expected_rev" ]]; then
  echo "Codex compatibility review required: expected $expected_rev, found $actual_rev" >&2
  git -C "$repo" diff --stat "$expected_rev..$actual_rev" -- \
    codex-rs/login/src/auth/manager.rs \
    codex-rs/login/src/auth/storage.rs \
    codex-rs/login/src/token_data.rs \
    codex-rs/login/src/auth/default_client.rs \
    codex-rs/codex-api/src/endpoint/responses.rs >&2
  exit 1
fi

echo "Codex compatibility source matches $expected_rev"
echo "Review these files when updating the pinned revision:"
echo "  codex-rs/login/src/auth/manager.rs"
echo "  codex-rs/login/src/auth/storage.rs"
echo "  codex-rs/login/src/token_data.rs"
echo "  codex-rs/login/src/auth/default_client.rs"
echo "  codex-rs/codex-api/src/endpoint/responses.rs"
