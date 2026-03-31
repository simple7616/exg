#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

export DATABASE_URL="${DATABASE_URL:-postgresql://exg:exg_dev_password@localhost:5432/exg}"

GREEN='\033[0;32m'; NC='\033[0m'
info() { echo -e "${GREEN}[INFO]${NC} $1"; }

command -v sqlx >/dev/null 2>&1 || { echo "Install sqlx-cli: cargo install sqlx-cli --no-default-features --features postgres"; exit 1; }

ACTION="${1:-status}"

case "$ACTION" in
    up|run)
        info "Running migrations..."
        sqlx migrate run --source migrations/
        ;;
    down|revert)
        info "Reverting last migration..."
        sqlx migrate revert --source migrations/
        ;;
    status|info)
        info "Migration status:"
        sqlx migrate info --source migrations/
        ;;
    reset)
        info "Resetting database..."
        sqlx database drop -y 2>/dev/null || true
        sqlx database create
        sqlx migrate run --source migrations/
        info "Database reset complete."
        ;;
    *)
        echo "Usage: $0 {up|down|status|reset}"
        echo "  up/run     - Apply pending migrations"
        echo "  down/revert - Revert last migration"
        echo "  status     - Show migration status"
        echo "  reset      - Drop and recreate database"
        exit 1
        ;;
esac
