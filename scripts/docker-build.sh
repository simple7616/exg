#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
cd "$PROJECT_ROOT"

TAG="${1:-latest}"
REGISTRY="${REGISTRY:-exg}"

GREEN='\033[0;32m'; NC='\033[0m'
info() { echo -e "${GREEN}[BUILD]${NC} $1"; }

info "Building exg-server:${TAG}..."
docker build -t "${REGISTRY}/server:${TAG}" -f Dockerfile .

info "Building exg-trading:${TAG}..."
docker build -t "${REGISTRY}/trading:${TAG}" -f Dockerfile.trading .

info "Building exg-admin:${TAG}..."
docker build -t "${REGISTRY}/admin:${TAG}" -f Dockerfile.admin .

echo ""
info "Images built:"
docker images | grep "^${REGISTRY}" | head -10
