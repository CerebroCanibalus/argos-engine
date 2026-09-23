#!/usr/bin/env bash
# Provision (default) or start (--up) the local Docker Engine + SearXNG stack inside WSL2.
# Fallback when Docker Desktop cannot be installed (Windows builds below 19045, e.g. LTSC 21H2).
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
cd "$(dirname "$0")"

start_stack() {
  echo "[start] docker daemon..."
  service docker start >/dev/null 2>&1 || true
  sleep 1
  echo "[start] SearXNG stack..."
  docker-compose up -d
  echo "[start] waiting for the JSON API on 127.0.0.1:8080..."
  for i in $(seq 1 45); do
    if curl -fsS "http://127.0.0.1:8080/search?q=ping&format=json" >/dev/null 2>&1; then
      echo "[ok] SearXNG JSON API answering"
      return 0
    fi
    sleep 2
  done
  echo "[error] SearXNG did not answer in ~90s; check: docker-compose logs" >&2
  return 1
}

if [ "${1:-}" = "--up" ]; then
  start_stack
  exit $?
fi

echo "[1/3] Installing Docker Engine + curl..."
apt-get update -qq
apt-get install -y -qq docker.io curl ca-certificates

echo "[2/3] Installing docker-compose v2 standalone..."
if ! command -v docker-compose >/dev/null 2>&1; then
  curl -fsSL https://github.com/docker/compose/releases/latest/download/docker-compose-linux-x86_64 \
    -o /usr/local/bin/docker-compose
  chmod +x /usr/local/bin/docker-compose
fi

echo "[3/3] Starting the stack..."
start_stack
