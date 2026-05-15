#!/usr/bin/env bash
# Stage 1b cold-boot demo: place → kill → reboot replays → wal-dump.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage1b.XXXXXX)
PORT=8080
SERVER_PID=""
TMP_CFG=$(mktemp /tmp/exg-stage1b-cfg.XXXXXX.toml)

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${WAL_DIR}"
    rm -f "${TMP_CFG}"
}
trap cleanup EXIT

start_server() {
    EXG_CONFIG="$TMP_CFG" RUST_LOG=info ./target/release/exg-server &
    SERVER_PID=$!
    for i in {1..30}; do
        if curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null; then
            return 0
        fi
        sleep 1
    done
    echo "server did not become ready" >&2
    return 1
}

stop_server() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}"
        wait "${SERVER_PID}" 2>/dev/null || true
        SERVER_PID=""
    fi
}

echo "── stage 1b demo ──"
docker compose up -d postgres
sleep 2

echo "─ migrate ─"
scripts/migrate.sh reset

echo "─ build ─"
cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "─ prepare config ─"
cp config/default.toml "$TMP_CFG"
python3 - <<PY
import re
with open('$TMP_CFG') as f: c = f.read()
c = re.sub(r'dir = "\\./data/wal"', f'dir = "$WAL_DIR"', c)
c = re.sub(r'jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"', 'jwt_secret = "demo-stage1b-secret-padding-32-bytes"', c)
with open('$TMP_CFG', 'w') as f: f.write(c)
PY

echo
echo "─ boot 1: register + login + place ─"
start_server

curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}' >/dev/null

LOGIN_RESP=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}')
TOKEN=$(echo "${LOGIN_RESP}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["accessToken"])')

curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H "Authorization: Bearer $TOKEN" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000","clientOrderId":"42"}'
echo

echo
echo "─ shutdown 1 ─"
stop_server

echo
echo "─ WAL contents after boot 1 ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | head -20
echo

echo
echo "─ boot 2: server replays from WAL ─"
start_server

echo "─ health check ─"
curl -sf "http://127.0.0.1:${PORT}/api/v1/health"
echo

echo
echo "─ shutdown 2 ─"
stop_server

echo
echo "─ WAL contents after boot 2 (no new events expected) ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | head -20
echo

echo "─ demo complete ─"
