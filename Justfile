default:
    @just --list

# Install system deps and Rust toolchain (run once, requires root)
setup:
    bash setup.sh

# Run the bot in development mode (unoptimized, fast compile)
dev:
    OPUS_STATIC=0 cargo run

# Build and run optimized release binary
run:
    OPUS_STATIC=0 cargo build --release
    ./target/release/writey

format:
    cargo fmt

clippy:
    cargo clippy

check:
    cargo check
