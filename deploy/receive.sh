#!/bin/sh
set -eu
target="${1:-test}"
case "$target" in
    test)
        bin="/opt/ktk-schedule/ktk-schedule-test"
        service="ktk-schedule-test.service"
        ;;
    prod)
        bin="/opt/ktk-schedule/ktk-schedule"
        service="ktk-schedule.service"
        ;;
    *)
        echo "Unknown deploy target: $target (expected 'test' or 'prod')" >&2
        exit 1
        ;;
esac

umask 077
exec 9>/run/lock/ktk-schedule-deploy.lock
flock -w 120 9
next="${bin}.next"
trap 'rm -f "$next"' EXIT HUP INT TERM

python3 -c '
import sys
limit = 100 * 1024 * 1024
total = 0
with open(sys.argv[1], "wb") as output:
    while chunk := sys.stdin.buffer.read(1024 * 1024):
        total += len(chunk)
        if total > limit:
            raise SystemExit("Binary exceeds 100 MiB")
        output.write(chunk)
    if total == 0:
        raise SystemExit("Empty binary")
' "$next"
chmod 755 "$next"
timeout 15 runuser -u ktk-schedule -- "$next" --version
mv "$next" "$bin"

systemctl restart "$service"
sleep 3
systemctl is-active --quiet "$service"
test "$(systemctl show "$service" -p NRestarts --value)" = 0
printf 'KTK deployed and running (%s on %s)\n' "$bin" "$service"
