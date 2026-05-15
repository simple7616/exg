#!/usr/bin/env bash
# Stage 2 demo: place stop → admin inject mark crosses stop → wal-dump
# fill → admin funding-tick → wal-dump rate → reboot replays.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage2.XXXXXX)
PORT=8080
ADMIN_PORT=9090
SERVER_PID=""
TMP_CFG=$(mktemp /tmp/exg-stage2-cfg.XXXXXX.toml)
ADMIN_SECRET="demo-stage2-admin-secret-32-bytes-ok"

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
        curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null && return 0
        sleep 1
    done
    echo "server not ready" >&2; return 1
}
stop_server() {
    [[ -n "${SERVER_PID}" ]] && { kill -INT "${SERVER_PID}"; wait "${SERVER_PID}" 2>/dev/null || true; SERVER_PID=""; }
}

echo "── stage 2 demo ──"
docker compose up -d postgres
sleep 2
echo "─ migrate ─"; scripts/migrate.sh reset
echo "─ build ─"; cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "─ prepare config ─"
cp config/default.toml "$TMP_CFG"
python3 - <<PY
import re
with open('$TMP_CFG') as f: c = f.read()
c = re.sub(r'dir = "\\./data/wal"', f'dir = "$WAL_DIR"', c)
c = re.sub(r'jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"', 'jwt_secret = "demo-stage2-jwt-secret-32-bytes-okk"', c)
c = re.sub(r'admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES"', 'admin_secret = "$ADMIN_SECRET"', c)
with open('$TMP_CFG','w') as f: f.write(c)
PY

echo
echo "─ boot 1 ─"; start_server

TOK=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" -H 'Content-Type: application/json' -d '{"email":"demo2@example.com","password":"hunter2hunter2"}' >/dev/null; \
      curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" -H 'Content-Type: application/json' -d '{"email":"demo2@example.com","password":"hunter2hunter2"}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["accessToken"])')

echo "─ rest a buy limit @59000 ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" -H "Authorization: Bearer $TOK" -H 'Content-Type: application/json' \
  -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000"}'; echo

echo "─ place STOP_MARKET sell, stop @59000 ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" -H "Authorization: Bearer $TOK" -H 'Content-Type: application/json' \
  -d '{"symbol":"BTCUSDT","side":"SELL","orderType":"STOP_MARKET","timeInForce":"GTC","quantity":"0.001","stopPrice":"59000"}'; echo

echo "─ admin inject mark price 58000 (crosses stop) ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/mark-price" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' \
  -d '{"markPrice":"58000","indexPrice":"58000"}'; echo

echo "─ admin funding-tick ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/funding-tick" -H "X-Admin-Secret: $ADMIN_SECRET"; echo

sleep 1
echo "─ shutdown 1 ─"; stop_server

echo
echo "─ WAL after boot 1 (expect MarkPriceUpdate, OrderFilled, FundingRateUpdate) ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | tail -25

echo
echo "─ boot 2: replay ─"; start_server
echo "─ health ─"; curl -sf "http://127.0.0.1:${PORT}/api/v1/health"; echo
echo "─ shutdown 2 ─"; stop_server
echo "─ demo complete ─"
