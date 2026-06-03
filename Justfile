golangci_lint_version := "v2.12.2"
govulncheck_version := "v1.3.0"

fmt:
    go fmt ./...

tidy-check:
    go mod tidy -diff

vet:
    go vet ./...

build:
    go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot

test:
    go test -count=1 ./...

lint:
    golangci-lint run

race:
    go test -count=1 -race ./...

vuln:
    @if command -v govulncheck >/dev/null 2>&1; then \
      govulncheck ./...; \
    else \
      go run golang.org/x/vuln/cmd/govulncheck@{{govulncheck_version}} ./...; \
    fi

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
    #!/usr/bin/env bash
    if [ -f .env ]; then
      for key in BOT_TOKEN CREDENTIALS_SECRET; do
        value=$(grep -E "^${key}=" .env | tail -n 1 | cut -d= -f2-)
        [ -n "$value" ] || { echo "error: $key is empty or missing in .env"; exit 1; }
      done
    fi

check: env-check tidy-check fmt vet lint test build

ci-check: tidy-check vet lint race vuln build

clean:
    rm -f ktk-schedule ktk-schedule.db *.log

backup:
    BACKUP_VOLUME_NAME=ktk-schedule_ktk_schedule_data BACKUP_DIR=backups scripts/backup-sqlite.sh

setup:
    git config core.hooksPath .githooks
    @echo "pre-commit hooks configured"

setup-air:
    go install github.com/air-verse/air@latest

setup-lint:
    go install github.com/golangci/golangci-lint/v2/cmd/golangci-lint@{{golangci_lint_version}}

setup-vuln:
    go install golang.org/x/vuln/cmd/govulncheck@{{govulncheck_version}}
