#!/bin/sh
set -eu

auth_file="${CODEX_HOME:-/root/.codex}/auth.json"
if [ ! -s "$auth_file" ]; then
    mkdir -p "$(dirname "$auth_file")"
    cat >&2 <<'EOF'
Codex credentials not found.
In another terminal, run:

  docker compose -f docker/docker-compose.yml exec codex2api \
    codex -c 'cli_auth_credentials_store="file"' login --device-auth

Waiting for login to complete...
EOF
    trap 'exit 0' INT TERM
    while [ ! -s "$auth_file" ]; do
        sleep 2 &
        wait $!
    done
    trap - INT TERM
    echo "Codex login detected; starting." >&2
fi

exec "$@"
