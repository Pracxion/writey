default:
    @just --list

setup:
    bash setup.sh

dev:
    OPUS_STATIC=0 cargo run

run:
    OPUS_STATIC=0 cargo build --release
    ./target/release/writey

format:
    cargo fmt

clippy:
    cargo clippy

check:
    cargo check
