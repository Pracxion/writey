#!/bin/bash
set -euo pipefail

if [ ! -f .env ]; then
    if [ -f .env.example ]; then
        cp .env.example .env
        echo "Created .env from env.example"
    else
        echo "Warning: .env.example not found, skipping .env creation"
    fi
else
    echo ".env already exists, skipping copy"
fi

cargo build
