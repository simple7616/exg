#!/usr/bin/env bash
# Stage 0 cold-boot demo. Spec §5.3.
set -euo pipefail

WAL_DIR=$(mktemp -d /tmp/exg-stage0.XXXXXX)
CONFIG_FILE=$(mktemp /tmp/exg-config.XXXXXX.toml)
PORT=8080
ADMIN_PORT=9099
SERVER_PID=""

cleanup() {
    if [[ -n "${SERVER_PID}" ]]; then
        kill -INT "${SERVER_PID}" 2>/dev/null || true
        wait "${SERVER_PID}" 2>/dev/null || true
    fi
    rm -rf "${WAL_DIR}"
    rm -f "${CONFIG_FILE}"
}
trap cleanup EXIT

echo "── stage 0 demo ──"
echo "WAL dir: ${WAL_DIR}"
echo "Building release binaries..."
cargo build --release -p exg-server -p exg-wal-dump >/dev/null

# Create a temp config with our WAL dir and a unique admin port
cp config/default.toml "${CONFIG_FILE}"
sed -i '' "s|dir = \".*\"|dir = \"${WAL_DIR}\"|" "${CONFIG_FILE}"
sed -i '' "s|admin_port = [0-9]*|admin_port = ${ADMIN_PORT}|" "${CONFIG_FILE}"

echo "Starting exg-server..."
EXG_CONFIG="${CONFIG_FILE}" \
    RUST_LOG=info \
    ./target/release/exg-server &
SERVER_PID=$!

# Wait up to 30s for health.
for i in {1..30}; do
    if curl -sf "http://127.0.0.1:${PORT}/api/v1/health" >/dev/null; then
        echo "server ready"
        break
    fi
    sleep 1
done

echo
echo "── place LIMIT buy ──"
RESP=$(curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order" \
    -H 'X-User-Id: 42' \
    -H 'Content-Type: application/json' \
    -d '{"symbol":"BTCUSDT","side":"BUY","orderType":"LIMIT","timeInForce":"GTC","quantity":"0.001","price":"59000"}')
echo "${RESP}"
ORDER_ID=$(echo "${RESP}" | python3 -c 'import json,sys; print(json.load(sys.stdin)["orderId"])')

echo
echo "── amend order ${ORDER_ID} ──"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order/amend" \
    -H 'X-User-Id: 42' \
    -H 'Content-Type: application/json' \
    -d "{\"orderId\":${ORDER_ID},\"symbol\":\"BTCUSDT\",\"newPrice\":\"59500\"}"
echo

echo
echo "── cancel order ${ORDER_ID} ──"
curl -s -X POST "http://127.0.0.1:${PORT}/api/v1/order/cancel" \
    -H 'X-User-Id: 42' \
    -H 'Content-Type: application/json' \
    -d "{\"orderId\":${ORDER_ID},\"symbol\":\"BTCUSDT\"}"
echo

echo
echo "── shutting down ──"
kill -INT "${SERVER_PID}"
wait "${SERVER_PID}" 2>/dev/null || true
SERVER_PID=""

echo
echo "── WAL contents ──"
./target/release/exg-wal-dump --wal-dir "${WAL_DIR}"
echo
echo "── demo complete ──"
