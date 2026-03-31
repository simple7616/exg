# Stage 1: Build
FROM rust:1.84-bookworm AS builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --workspace

# Stage 2: Runtime
FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/exg-server /usr/local/bin/
COPY config/ /etc/exg/
ENTRYPOINT ["exg-server"]
