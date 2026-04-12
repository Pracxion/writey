#!/bin/bash
set -euo pipefail

echo "Installing system dependencies..."
pacman -Sy --noconfirm \
    opus \
    pkg-config \
    openssl \
    base-devel \
    ca-certificates \
    curl

echo "Setting OPUS_STATIC=0 in .env..."
if grep -q "OPUS_STATIC" .env 2>/dev/null; then
    sed -i 's/^OPUS_STATIC=.*/OPUS_STATIC=0/' .env
elif [ -f .env ]; then
    echo "OPUS_STATIC=0" >> .env
fi

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
echo "Note: uses system Opus library (OPUS_STATIC=0)"
