# Docker commands
default:
    @just --list

# Start containers in detached mode
up:
    docker compose up -d

# Stop and remove containers
down:
    docker compose down

# View logs
logs:
    docker compose logs -f

# Restart containers
restart:
    docker compose restart

# Stop containers (without removing)
stop:
    docker compose stop

# Start stopped containers
start:
    docker compose start

# Show container status
ps:
    docker compose ps

# Execute command in container
exec cmd:
    docker compose exec writey {{cmd}}

# View container logs (tail)
tail lines="100":
    docker compose logs --tail={{lines}} -f

# Watch for Rust changes and rebuild/run in container
# Usage: just watch [build|run] (default: build)
watch mode="build":
    @if [ "{{mode}}" = "build" ]; then \
        docker compose up dev-build; \
    elif [ "{{mode}}" = "run" ]; then \
        docker compose up dev-run; \
    else \
        echo "Invalid mode. Use 'build' or 'run'"; \
        exit 1; \
    fi

format:
    cargo fmt

clippy:
    cargo clippy

check:
    cargo check

