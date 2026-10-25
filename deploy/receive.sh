#!/bin/sh
set -eu
test "$#" = 0
umask 077
exec 9>/run/lock/ktk-schedule-deploy.lock
flock -w 120 9
next=/opt/ktk-schedule/ktk-schedule.next
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
mv "$next" /opt/ktk-schedule/ktk-schedule

if systemctl is-active --quiet ktk-schedule-test.service; then
    target=ktk-schedule-test.service
else
    target=ktk-schedule.service
fi

systemctl restart "$target"
sleep 3
systemctl is-active --quiet "$target"
test "$(systemctl show "$target" -p NRestarts --value)" = 0
printf 'KTK deployed and running (%s)\n' "$target"
