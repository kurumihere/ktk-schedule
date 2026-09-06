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
systemctl restart ktk-schedule.service
sleep 3
systemctl is-active --quiet ktk-schedule.service
test "$(systemctl show ktk-schedule.service -p NRestarts --value)" = 0
printf 'KTK deployed and running\n'
