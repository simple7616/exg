.PHONY: all setup dev test bench lint build clean check migrate

# Default target
all: check test lint

# First-time setup
setup:
	@bash scripts/setup.sh

# Start development environment
dev:
	@bash scripts/dev.sh

# Run all tests
test:
	@bash scripts/test.sh

# Run tests including frontend builds
test-all:
	@bash scripts/test.sh --all

# Run benchmarks
bench:
	@bash scripts/bench.sh

# Run linters
lint:
	@bash scripts/lint.sh

# Cargo check
check:
	cargo check --workspace

# Build release binaries
build:
	cargo build --release --workspace

# Build Docker images
docker:
	@bash scripts/docker-build.sh

# Database migrations
migrate:
	@bash scripts/migrate.sh up

migrate-down:
	@bash scripts/migrate.sh down

migrate-reset:
	@bash scripts/migrate.sh reset

migrate-status:
	@bash scripts/migrate.sh status

# Start frontend dev servers
dev-trading:
	cd web/trading && npm run dev

dev-admin:
	cd web/admin && npm run dev -- --port 3001

# Clean build artifacts
clean:
	cargo clean
	rm -rf web/trading/.next web/admin/.next
	rm -rf web/trading/node_modules web/admin/node_modules

# Format code
fmt:
	cargo fmt
