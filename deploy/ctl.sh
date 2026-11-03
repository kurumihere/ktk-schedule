#!/bin/sh
# Manage the bot on the server from a workstation:
#   deploy/ctl.sh status
#   deploy/ctl.sh install      (copy deploy/ to the host and run install.sh there)
#   deploy/ctl.sh logs|follow|start|stop|restart|backup [prod|test]
# The SSH host alias defaults to "vps" (override with KTK_SSH_HOST).
set -eu

host="${KTK_SSH_HOST:-vps}"
action="${1:-status}"
instance="${2:-prod}"
usage() {
    echo "usage: $0 status | install | logs|follow|start|stop|restart|backup [prod|test]" >&2
    exit 64
}
case "$instance" in prod | test) ;; *) usage ;; esac
unit="ktk-schedule@$instance.service"

case "$action" in
    install)
        # Units, deploy and backup tools; install.sh never starts or upgrades a bot.
        tar -C "$(dirname "$0")" -cf - . |
            ssh "$host" 'dir=$(mktemp -d) && tar -C "$dir" -xf - && sh "$dir/install.sh"; status=$?; rm -rf "$dir"; exit $status'
        ;;
    status)
        ssh "$host" "systemctl list-units --all --no-pager 'ktk-schedule*'; systemctl list-timers --no-pager 'ktk-schedule-backup*'"
        ;;
    logs) ssh "$host" "journalctl -u $unit -n 200 --no-pager -o short-iso" ;;
    follow) ssh -t "$host" "journalctl -u $unit -f -o short-iso" ;;
    start | stop | restart)
        ssh "$host" "systemctl $action $unit; systemctl status $unit --no-pager --lines=10"
        ;;
    backup)
        ssh "$host" "systemctl start ktk-schedule-backup@$instance.service && ls -lt /var/backups/ktk-schedule/$instance | head -5"
        ;;
    *) usage ;;
esac
