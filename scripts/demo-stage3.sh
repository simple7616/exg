#!/usr/bin/env bash
# Stage 3 demo: admin-credit 2 users → cross LIMIT orders open opposing
# positions → admin mark-price → admin funding-tick settles → wal-dump
# shows AdminCredited + FundingRateUpdate + FundingSettled → reboot replays.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage3.XXXXXX)
PORT=8080
ADMIN_PORT=9090
SERVER_PID=""
TMP_CFG=$(mktemp /tmp/exg-stage3-cfg.XXXXXX.toml)
ADMIN_SECRET="demo-stage3-admin-secret-32-bytes-ok"
JWT_SECRET="demo-stage3-jwt-secret-32-bytes-okk"

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

# Decode the `user_id` claim from a JWT (no signature check — same id the
# order handler resolves via verify_jwt(...).user_id).
jwt_user_id() {
    python3 - "$1" <<'PY'
import sys, json, base64
tok = sys.argv[1]
payload = tok.split('.')[1]
payload += '=' * (-len(payload) % 4)
print(json.loads(base64.urlsafe_b64decode(payload))["user_id"])
PY
}

login_token() {
    local email="$1"
    curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/register" -H 'Content-Type: application/json' \
        -d "{\"email\":\"${email}\",\"password\":\"hunter2hunter2\"}" >/dev/null
    curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/auth/login" -H 'Content-Type: application/json' \
        -d "{\"email\":\"${email}\",\"password\":\"hunter2hunter2\"}" \
        | python3 -c 'import json,sys;print(json.load(sys.stdin)["accessToken"])'
}

echo "── stage 3 demo ──"
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
c = re.sub(r'jwt_secret = "CHANGE-ME-DEV-ONLY-MUST-BE-AT-LEAST-32-BYTES-OK"', 'jwt_secret = "$JWT_SECRET"', c)
c = re.sub(r'admin_secret = "CHANGE-ME-ADMIN-DEV-ONLY-MUST-BE-32-BYTES"', 'admin_secret = "$ADMIN_SECRET"', c)
with open('$TMP_CFG','w') as f: f.write(c)
PY

echo
echo "─ boot 1 ─"; start_server

echo "─ register + login user1, user2 ─"
T1=$(login_token "s3demo-a@example.com")
T2=$(login_token "s3demo-b@example.com")
U1=$(jwt_user_id "$T1")
U2=$(jwt_user_id "$T2")
echo "  user1 id=$U1  user2 id=$U2"

echo "─ admin credit both users 100000 ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/credit" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' -d "{\"userId\":${U1},\"amount\":\"100000\"}"; echo
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/credit" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' -d "{\"userId\":${U2},\"amount\":\"100000\"}"; echo

echo "─ user1 BUY 1 @60000, user2 SELL 1 @60000 (cross) ─"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" -H "Authorization: Bearer $T1" -H 'Content-Type: application/json' \
  -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}'; echo
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" -H "Authorization: Bearer $T2" -H 'Content-Type: application/json' \
  -d '{"symbol":"BTCUSDT","side":"SELL","orderType":"LIMIT","timeInForce":"GTC","quantity":"1","price":"60000"}'; echo

echo "─ admin mark-price 60000 ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/mark-price" -H "X-Admin-Secret: $ADMIN_SECRET" -H 'Content-Type: application/json' -d '{"markPrice":"60000","indexPrice":"60000"}'; echo

echo "─ admin funding-tick ─"
curl -s -X POST "http://127.0.0.1:${ADMIN_PORT}/api/v1/admin/funding-tick" -H "X-Admin-Secret: $ADMIN_SECRET"; echo

sleep 1
echo "─ shutdown 1 ─"; stop_server

echo
echo "─ WAL (expect AdminCredited, FundingRateUpdate, FundingSettled) ─"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}" | tail -30

echo
echo "─ boot 2: replay ─"; start_server
echo "─ health ─"; curl -sf "http://127.0.0.1:${PORT}/api/v1/health"; echo
echo "─ shutdown 2 ─"; stop_server
echo "─ demo complete ─"
