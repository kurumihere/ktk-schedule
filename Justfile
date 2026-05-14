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

check: fmt vet test build

clean:
    rm -f ktk-schedule ktk-schedule.db *.log

setup:
    git config core.hooksPath .githooks
    @echo "pre-commit hooks configured"

setup-air:
    go install github.com/air-verse/air@latest

setup-lint:
    go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest
