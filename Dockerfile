FROM golang:1.23-alpine AS builder

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

WORKDIR /app

RUN adduser -D -H -s /sbin/nologin app

COPY --from=builder /out/ktk-schedule /app/ktk-schedule

USER app

CMD ["/app/ktk-schedule"]