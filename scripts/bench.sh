#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

echo "EXG Benchmark Suite"
echo "━━━━━━━━━━━━━━━━━━━"
echo ""

CRATE=${1:-all}

if [ "$CRATE" = "all" ] || [ "$CRATE" = "decimal" ]; then
    echo "▶ Decimal128 arithmetic"
    cargo bench -p exg-common --bench decimal128 2>&1 | grep -E "time:|thrpt:" || true
    echo ""
fi

if [ "$CRATE" = "all" ] || [ "$CRATE" = "matching" ]; then
    echo "▶ Matching engine"
    cargo bench -p exg-matching-engine --bench matching 2>&1 | grep -E "time:|thrpt:" || true
    echo ""
fi

if [ "$CRATE" = "all" ] || [ "$CRATE" = "ringbuffer" ]; then
    echo "▶ Ring buffer"
    cargo bench -p exg-ringbuffer --bench ringbuffer 2>&1 | grep -E "time:|thrpt:" || true
    echo ""
fi

if [ "$CRATE" = "all" ] || [ "$CRATE" = "wal" ]; then
    echo "▶ Write-Ahead Log"
    cargo bench -p exg-wal --bench wal 2>&1 | grep -E "time:|thrpt:" || true
    echo ""
fi

echo "Done. Full HTML reports in target/criterion/"
