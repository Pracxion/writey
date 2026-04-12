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

echo "Setting up .env..."
if [ ! -f .env ]; then
    cp .env.example .env
    echo "Created .env from .env.example — fill in DISCORD_TOKEN and GUILD_ID"
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
