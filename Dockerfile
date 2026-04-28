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

RUN apk add --no-cache tzdata

WORKDIR /app

COPY --from=builder /out/ktk-schedule /app/ktk-schedule

CMD ["/app/ktk-schedule"]