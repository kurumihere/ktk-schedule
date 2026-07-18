FROM golang:1.26.5-alpine3.24 AS builder

WORKDIR /src

ENV GOTOOLCHAIN=local

COPY go.mod go.sum ./
RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked \
    go mod download

COPY cmd ./cmd
COPY internal ./internal

ARG TARGETOS
ARG TARGETARCH

RUN --mount=type=cache,target=/go/pkg/mod,sharing=locked \
    --mount=type=cache,target=/root/.cache/go-build \
    CGO_ENABLED=0 GOOS=$TARGETOS GOARCH=$TARGETARCH go build \
    -buildvcs=false \
    -trimpath \
    -ldflags="-s -w" \
    -o /out/ktk-schedule \
    ./cmd/bot

FROM alpine:3.24.1

LABEL org.opencontainers.image.source="https://github.com/kurumihere/ktk-schedule" \
      org.opencontainers.image.licenses="BSD-3-Clause"

RUN apk add --no-cache ca-certificates tzdata \
    && addgroup -g 1000 app \
    && adduser -D -H -h /app -u 1000 -G app app \
    && mkdir -p /app/data \
    && chown app:app /app/data

WORKDIR /app

COPY --from=builder --chown=app:app /out/ktk-schedule /app/ktk-schedule
COPY --chown=app:app LICENSE /app/LICENSE
USER app

STOPSIGNAL SIGTERM

CMD ["/app/ktk-schedule"]
