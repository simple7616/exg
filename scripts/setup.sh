#!/usr/bin/env bash
set -euo pipefail

# Colors
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

info "Setting up EXG development environment..."

# Check prerequisites
command -v cargo >/dev/null 2>&1 || error "Rust/Cargo not found. Install from https://rustup.rs"
command -v node >/dev/null 2>&1 || error "Node.js not found. Install from https://nodejs.org"
command -v docker >/dev/null 2>&1 || warn "Docker not found. Required for local services."

# Rust setup
info "Building Rust workspace..."
cargo check --workspace

# Frontend setup
info "Installing trading frontend dependencies..."
(cd web/trading && npm ci)

info "Installing admin frontend dependencies..."
(cd web/admin && npm ci)

# Docker services
if command -v docker >/dev/null 2>&1; then
    info "Starting infrastructure services (PostgreSQL, Redis, NATS)..."
    docker compose up -d postgres redis nats
    info "Waiting for PostgreSQL..."
    until docker compose exec -T postgres pg_isready -U exg >/dev/null 2>&1; do sleep 1; done

    # Run migrations if sqlx-cli available
    if command -v sqlx >/dev/null 2>&1; then
        info "Running database migrations..."
        export DATABASE_URL="postgresql://exg:exg_dev_password@localhost:5432/exg"
        sqlx migrate run --source migrations/
    else
        warn "sqlx-cli not found. Install with: cargo install sqlx-cli"
        warn "Then run: DATABASE_URL=postgresql://exg:exg_dev_password@localhost:5432/exg sqlx migrate run --source migrations/"
    fi
fi

info "Setup complete!"
echo ""
echo "Quick start:"
echo "  cargo test --workspace          # Run all tests"
echo "  scripts/dev.sh                  # Start dev environment"
echo "  scripts/test.sh                 # Run tests with coverage"
