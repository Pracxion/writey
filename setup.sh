#!/bin/bash
set -euo pipefail

echo "Installing system dependencies..."
pacman -Sy --noconfirm \
    opus \
    pkg-config \
    openssl \
    base-devel \
    ca-certificates \
    curl \
    mold \
    clang \
    flac

echo "Installing Rust toolchain..."
if ! command -v cargo &>/dev/null; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
    export PATH="$HOME/.cargo/bin:$PATH"
else
    echo "Rust already installed: $(cargo --version)"
fi

echo ""
echo "Setup complete. Run the bot with:"
echo "  source \$HOME/.cargo/env && cargo run"
echo "  or: just dev"
echo ""
