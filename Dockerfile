FROM golang:1.26-alpine AS builder

WORKDIR /src

COPY go.mod go.sum ./
RUN go mod download

COPY . .

RUN CGO_ENABLED=0 GOOS=linux go build \
    -trimpath \
    -ldflags="-s -w" \
    -o /out/ktk-schedule \
    ./cmd/bot

FROM alpine:3.22

RUN apk add --no-cache tzdata curl su-exec \
    && adduser -D -h /app app \
    && mkdir -p /app/data

WORKDIR /app

COPY --from=builder --chown=app:app /out/ktk-schedule /app/ktk-schedule
COPY docker-entrypoint.sh /docker-entrypoint.sh
RUN chmod +x /docker-entrypoint.sh

ENTRYPOINT ["/docker-entrypoint.sh"]

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:8080/health || exit 1

CMD ["/app/ktk-schedule"]
