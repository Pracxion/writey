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
