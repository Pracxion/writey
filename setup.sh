#!/bin/bash
set -euo pipefail

# Copy .env.example to .env if needed
if [ ! -f .env ] && [ -f .env.example ]; then
    cp .env.example .env
fi

docker compose up -d