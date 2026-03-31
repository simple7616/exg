#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info() { echo -e "${GREEN}[INFO]${NC} $1"; }
error() { echo -e "${RED}[FAIL]${NC} $1"; }
FAILED=0

# Parse args
RUN_FRONTEND=false
RUN_BENCH=false
VERBOSE=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --frontend) RUN_FRONTEND=true; shift ;;
        --bench) RUN_BENCH=true; shift ;;
        --verbose|-v) VERBOSE=true; shift ;;
        --all) RUN_FRONTEND=true; RUN_BENCH=true; shift ;;
        -h|--help)
            echo "Usage: $0 [--frontend] [--bench] [--all] [--verbose]"
            echo "  --frontend  Also build frontend projects"
            echo "  --bench     Also run benchmarks"
            echo "  --all       Run everything"
            echo "  --verbose   Show full output"
            exit 0 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

# 1. Cargo check
info "Running cargo check..."
if ! cargo check --workspace 2>&1; then
    error "cargo check failed"
    FAILED=1
fi

# 2. Clippy
info "Running clippy (deny warnings)..."
if ! cargo clippy --workspace -- -D warnings 2>&1; then
    error "clippy found warnings"
    FAILED=1
fi

# 3. Tests
info "Running tests..."
TEST_OUTPUT=$(cargo test --workspace 2>&1)
TEST_COUNT=$(echo "$TEST_OUTPUT" | grep "^test result" | awk '{sum += $4} END {print sum+0}')
FAIL_COUNT=$(echo "$TEST_OUTPUT" | grep "^test result" | awk '{sum += $6} END {print sum+0}')

if [ "$VERBOSE" = true ]; then
    echo "$TEST_OUTPUT"
fi

if [ "$FAIL_COUNT" -gt 0 ]; then
    error "Tests failed: $FAIL_COUNT failures out of $TEST_COUNT tests"
    FAILED=1
else
    info "All $TEST_COUNT tests passed"
fi

# 4. Frontend builds (optional)
if [ "$RUN_FRONTEND" = true ]; then
    info "Building trading frontend..."
    if ! (cd web/trading && npm run build) 2>&1; then
        error "Trading frontend build failed"
        FAILED=1
    fi

    info "Building admin frontend..."
    if ! (cd web/admin && npm run build) 2>&1; then
        error "Admin frontend build failed"
        FAILED=1
    fi
fi

# 5. Benchmarks (optional)
if [ "$RUN_BENCH" = true ]; then
    info "Running benchmarks..."
    cargo bench --workspace -- --output-format bencher 2>&1 | grep "bench:" || true
fi

# Summary
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
if [ "$FAILED" -eq 0 ]; then
    info "All checks passed"
else
    error "Some checks failed"
    exit 1
fi
