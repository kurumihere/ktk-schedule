fmt:
    go fmt ./...

vet:
    go vet ./...

test:
    go test ./...

build:
    go build -trimpath -ldflags="-s -w" -o ktk-schedule ./cmd/bot

run:
    go run ./cmd/bot

check: fmt vet test build

docker:
    docker compose up --build -d

down:
    docker compose down

logs:
    docker compose logs -f

clean:
    rm -f ktk-schedule ktk-schedule.db *.log

backup:
    BACKUP_VOLUME_NAME=ktk-schedule_ktk_schedule_data BACKUP_DIR=backups scripts/backup-sqlite.sh

setup:
    git config core.hooksPath .githooks
    @echo "git hooks configured"
