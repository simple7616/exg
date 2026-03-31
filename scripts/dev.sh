#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

# Colors
GREEN='\033[0;32m'; YELLOW='\033[1;33m'; NC='\033[0m'
info() { echo -e "${GREEN}[INFO]${NC} $1"; }

# Start infrastructure
info "Starting infrastructure services..."
docker compose up -d

# Wait for services
info "Waiting for services to be ready..."
until docker compose exec -T postgres pg_isready -U exg >/dev/null 2>&1; do sleep 1; done
until docker compose exec -T redis redis-cli ping >/dev/null 2>&1; do sleep 1; done
info "Infrastructure ready."

echo ""
echo "Services:"
echo "  PostgreSQL:  localhost:5432 (exg/exg_dev_password)"
echo "  Redis:       localhost:6379"
echo "  NATS:        localhost:4222 (monitoring: localhost:8222)"
echo "  Prometheus:  http://localhost:9090"
echo "  Grafana:     http://localhost:3100 (admin/admin)"
echo ""
echo "To start frontends:"
echo "  cd web/trading && npm run dev    # Trading UI: http://localhost:3000"
echo "  cd web/admin && npm run dev      # Admin UI:   http://localhost:3001"
echo ""
echo "To start the exchange server:"
echo "  cargo run -p exg-server"
echo ""
echo "To stop everything:"
echo "  docker compose down"
