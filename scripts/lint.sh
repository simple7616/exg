#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

GREEN='\033[0;32m'; RED='\033[0;31m'; NC='\033[0m'
FAILED=0

echo "Running linters..."
echo ""

echo "▶ cargo fmt --check"
if ! cargo fmt --check 2>&1; then
    echo -e "${RED}  Format check failed. Run: cargo fmt${NC}"
    FAILED=1
else
    echo -e "${GREEN}  Formatted${NC}"
fi
echo ""

echo "▶ cargo clippy"
if ! cargo clippy --workspace -- -D warnings 2>&1; then
    FAILED=1
else
    echo -e "${GREEN}  No clippy warnings${NC}"
fi
echo ""

echo "▶ TypeScript (trading)"
if [ -d web/trading ]; then
    if (cd web/trading && npx tsc --noEmit 2>&1); then
        echo -e "${GREEN}  Trading types OK${NC}"
    else
        FAILED=1
    fi
fi
echo ""

echo "▶ TypeScript (admin)"
if [ -d web/admin ]; then
    if (cd web/admin && npx tsc --noEmit 2>&1); then
        echo -e "${GREEN}  Admin types OK${NC}"
    else
        FAILED=1
    fi
fi
echo ""

if [ "$FAILED" -eq 0 ]; then
    echo -e "${GREEN}All linters passed${NC}"
else
    echo -e "${RED}Linting failed${NC}"
    exit 1
fi
