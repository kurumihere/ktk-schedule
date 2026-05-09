FROM golang:latest AS builder

WORKDIR /src

COPY go.mod go.sum ./
RUN go mod download

COPY . .

RUN CGO_ENABLED=0 GOOS=linux go build \
    -trimpath \
    -ldflags="-s -w" \
    -o /out/ktk-schedule \
    ./cmd/bot

FROM alpine:3.21

RUN apk add --no-cache tzdata curl

WORKDIR /app

COPY --from=builder /out/ktk-schedule /app/ktk-schedule

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -sf http://localhost:8080/health || exit 1

CMD ["/app/ktk-schedule"]