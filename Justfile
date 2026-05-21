fmt:
    go fmt ./...

vet:
    go vet ./...

build:
    go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot

test:
    go test -count=1 ./...

lint:
    golangci-lint run

run:
    go run ./cmd/bot

dev:
    air

docker:
    docker compose up --build -d

docker-down:
    docker compose down

docker-logs:
    docker compose logs -f

env-check:
    @if [ -f .env ]; then \
      grep -q '^BOT_TOKEN=' .env || { echo "error: BOT_TOKEN not set in .env"; exit 1; }; \
      grep -q '^CREDENTIALS_SECRET=' .env || { echo "error: CREDENTIALS_SECRET not set in .env"; exit 1; }; \
    fi

check: env-check fmt vet test build

clean:
    rm -f ktk-schedule ktk-schedule.db *.log

backup:
    cp ktk-schedule.db ktk-schedule-$(date +%Y%m%d-%H%M%S).db
    @echo "backup created"

setup:
    git config core.hooksPath .githooks
    @echo "pre-commit hooks configured"

setup-air:
    go install github.com/air-verse/air@latest

setup-lint:
    go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
