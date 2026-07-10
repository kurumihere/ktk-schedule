# syntax=docker/dockerfile:1.7

FROM golang:1.26-alpine AS builder

WORKDIR /src

ARG GOPROXY=https://proxy.golang.org,direct
ENV GOPROXY=$GOPROXY

COPY go.mod go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod \
    set -eux; \
    for attempt in 1 2 3; do \
        if go mod download; then \
            exit 0; \
        fi; \
        sleep "$((attempt * 5))"; \
    done; \
    go mod download

COPY . .

RUN --mount=type=cache,target=/go/pkg/mod \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 GOOS=linux go build \
    -buildvcs=false \
    -trimpath \
    -ldflags="-s -w" \
    -o /out/ktk-schedule \
    ./cmd/bot

FROM alpine:3.22

RUN apk add --no-cache tzdata curl \
    && adduser -D -h /app app \
    && mkdir -p /app/data \
    && chown app:app /app/data

WORKDIR /app

COPY --from=builder --chown=app:app /out/ktk-schedule /app/ktk-schedule
USER app

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:8080/health || exit 1

CMD ["/app/ktk-schedule"]
