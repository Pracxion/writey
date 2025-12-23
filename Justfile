# Docker commands
default:
    @just --list

# Start containers in detached mode
up:
    docker compose up -d

# Stop and remove containers
down:
    docker compose down

# Build Docker images
build:
    docker compose build

# Rebuild Docker images (no cache)
build-no-cache:
    docker compose build --no-cache

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

