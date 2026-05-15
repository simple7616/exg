#!/usr/bin/env bash
# Stage 1a cold-boot demo: PG up → migrate → server → register/login/order/dup → wal-dump.
# Spec §5.3 / §8.6.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage1a.XXXXXX)
TMP_CFG=$(mktemp /tmp/exg-stage1a-cfg.XXXXXX.toml)
PORT=8080
SERVER_PID=""

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${WAL_DIR}"
    rm -f "${TMP_CFG}"
}
trap cleanup EXIT

echo "── stage 1a demo ──"
docker compose up -d postgres
sleep 2

echo "─ migrate ─"
scripts/migrate.sh reset

echo "─ build ─"
cargo build --release -p exg-server -p exg-wal-dump >/dev/null

echo "─ boot server ─"
cp config/default.toml "${TMP_CFG}"
# Override WAL dir and JWT secret in the temp config.
python3 -c "
import re
p = '${TMP_CFG}'
with open(p) as f:
    c = f.read()
c = re.sub(r'dir = \"./data/wal\"', 'dir = \"${WAL_DIR}\"', c)
c = re.sub(
    r'jwt_secret = \"CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK\"',
    'jwt_secret = \"demo-stage1a-secret-padding-32-bytes\"',
    c,
)
with open(p, 'w') as f:
    f.write(c)
"

EXG_CONFIG="${TMP_CFG}" RUST_LOG=info ./target/release/exg-server &
SERVER_PID=$!

# Wait up to 30 s for health.
for i in {1..30}; do
    if curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null; then
        echo "server ready"
        break
    fi
    sleep 1
done

echo
echo "─ register ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}'
echo

echo
echo "─ login ─"
LOGIN_RESP=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" \
    -H 'Content-Type: application/json' \
    -d '{"email":"demo@example.com","password":"hunter2hunter2"}')
echo "${LOGIN_RESP}"
TOKEN=$(echo "${LOGIN_RESP}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["accessToken"])')

echo
echo "─ place order ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000","clientOrderId":"42"}'
echo

echo
echo "─ duplicate clientOrderId (should 409) ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H "Authorization: Bearer ${TOKEN}" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000","clientOrderId":"42"}'
echo

echo
echo "─ no token (should 401) ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000"}'
echo

echo
echo "─ /me ─"
curl -s -X GET "http://127.0.0.1:${PORT}/api/v1/me" \
    -H "Authorization: Bearer ${TOKEN}"
echo

echo
echo "─ shutdown ─"
kill -INT "${SERVER_PID}"
wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""

echo
echo "─ WAL contents ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}"
echo
echo "─ demo complete ─"
