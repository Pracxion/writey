FROM rustlang/rust:nightly-slim AS builder
WORKDIR /var/build
COPY . ./writey
WORKDIR /var/build/writey
RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    clang \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo build --release

FROM debian:bookworm-slim AS prod
WORKDIR /app
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libsqlite3-0 \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1000 appuser
COPY --from=builder /var/build/writey/target/release/writey ./writey
COPY --from=builder /var/build/writey/migrations ./migrations
RUN mkdir -p recordings settings models/whisper && chown -R appuser:appuser recordings settings models
USER appuser
CMD ["./writey"]

FROM rustlang/rust:nightly-slim AS dev-build
WORKDIR /app
RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    clang \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-watch
CMD ["cargo", "watch", "-x", "build"]

FROM rustlang/rust:nightly-slim AS dev-run
WORKDIR /app
RUN apt-get update && apt-get install -y \
    cmake \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    clang \
    libclang-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-watch
CMD ["cargo", "watch", "-x", "run"]

# ============================================
# CUDA-enabled build (for NVIDIA GPU support)
# ============================================
# Requires: nvidia-docker or docker with --gpus flag
# Build: docker-compose build writey-cuda
# Run: docker-compose up writey-cuda

FROM nvidia/cuda:12.2.0-devel-ubuntu22.04 AS cuda-builder
WORKDIR /var/build

# Install Rust and dependencies
RUN apt-get update && apt-get install -y \
    curl \
    cmake \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    clang \
    libclang-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
ENV PATH="/root/.cargo/bin:${PATH}"

# Copy source code
COPY . ./writey
WORKDIR /var/build/writey

# Build with CUDA support
RUN cargo build --release --features cuda

# Verify binary was built (should be ~50-400MB)
RUN ls -lh target/release/writey

FROM nvidia/cuda:12.2.0-runtime-ubuntu22.04 AS cuda-prod
WORKDIR /app
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    libsqlite3-0 \
    libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1000 appuser
COPY --from=cuda-builder /var/build/writey/target/release/writey ./writey
COPY --from=cuda-builder /var/build/writey/migrations ./migrations
RUN mkdir -p recordings settings models/whisper && chown -R appuser:appuser recordings settings models
USER appuser
CMD ["./writey"]
