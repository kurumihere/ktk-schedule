#!/bin/sh
# deploy/install.sh: provision or update a host for ktk-schedule. Idempotent; run as root
# from a checkout (or with this directory copied to the host).
#
#   - service user, directories, systemd units and backup timers, deploy/backup tools,
#     the sudo rule that lets the CI runner deploy
#   - per-instance settings /etc/ktk-schedule/<instance>/bot.env plus encrypted credentials
#     (bot_token, credentials_secret, vk_group_token), converted once from the pre-0.3
#     files /etc/ktk-schedule/bot.env (prod) and bot-test.env (test)
#   - the pre-0.3 database of an instance is copied once, only while its old unit is stopped
#
# It never starts, stops or upgrades a bot: ktk-schedule-deploy does that.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
[ "$(id -u)" = 0 ] || {
    echo "install.sh must run as root" >&2
    exit 1
}
say() { printf 'install: %s\n' "$*"; }

id ktk-schedule >/dev/null 2>&1 ||
    useradd --system --home-dir /var/lib/ktk-schedule --no-create-home \
        --shell /usr/sbin/nologin ktk-schedule
install -d -m 0700 -o ktk-schedule -g ktk-schedule /var/lib/ktk-schedule /var/backups/ktk-schedule
install -d -m 0700 /etc/ktk-schedule
install -d -m 0755 /opt/ktk-schedule

install -m 0644 "$here/ktk-schedule@.service" "$here/ktk-schedule-backup@.service" \
    "$here/ktk-schedule-backup@.timer" /etc/systemd/system/
install -m 0755 "$here/ktk-schedule-deploy" "$here/ktk-schedule-backup" /usr/local/sbin/
rule=/etc/sudoers.d/ktk-schedule-runner
printf 'ktk-runner ALL=(root) NOPASSWD: /usr/local/sbin/ktk-schedule-deploy *\n' >"$rule.new"
chmod 0440 "$rule.new"
visudo -cqf "$rule.new"
mv -f "$rule.new" "$rule"

# Value of KEY in an env file, without surrounding quotes.
setting() {
    sed -n "s/^$1=//p" "$2" | tail -n 1 | sed -e 's/^"\(.*\)"$/\1/' -e "s/^'\(.*\)'$/\1/"
}

for instance in prod test; do
    conf=/etc/ktk-schedule/$instance
    state=/var/lib/ktk-schedule/$instance
    case $instance in
        prod) legacy_env=/etc/ktk-schedule/bot.env legacy_unit=ktk-schedule.service ;;
        test) legacy_env=/etc/ktk-schedule/bot-test.env legacy_unit=ktk-schedule-test.service ;;
    esac
    install -d -m 0700 "$conf"
    install -d -m 0755 "/opt/ktk-schedule/$instance"
    install -d -m 0700 -o ktk-schedule -g ktk-schedule "$state" "/var/backups/ktk-schedule/$instance"

    if [ ! -f "$conf/bot.env" ] && [ -f "$legacy_env" ]; then
        # Secrets leave the plain-text file: they become credentials sealed with the host key.
        for key in BOT_TOKEN CREDENTIALS_SECRET VK_GROUP_TOKEN; do
            name=$(printf '%s' "$key" | tr 'A-Z' 'a-z')
            rm -f "$conf/$name.cred"
            setting "$key" "$legacy_env" | tr -d '\n' |
                systemd-creds encrypt --name="$name" - "$conf/$name.cred"
        done
        # DATABASE_PATH is the unit's business now.
        grep -vE '^[[:space:]]*(BOT_TOKEN|CREDENTIALS_SECRET|VK_GROUP_TOKEN|DATABASE_PATH)=' \
            "$legacy_env" >"$conf/bot.env.new"
        chmod 0600 "$conf/bot.env.new"
        mv -f "$conf/bot.env.new" "$conf/bot.env"
        say "$instance: settings converted from $legacy_env"
    fi

    # The old database moves once, and only while nothing writes to it.
    if [ -f "$legacy_env" ] && [ ! -f "$state/bot.sqlite3" ]; then
        legacy_db=$(setting DATABASE_PATH "$legacy_env")
        case $legacy_db in
            "" | /*) ;;
            *) legacy_db=/var/lib/ktk-schedule/$legacy_db ;;
        esac
        if [ -n "$legacy_db" ] && [ -f "$legacy_db" ]; then
            if systemctl is-active --quiet "$legacy_unit"; then
                say "$instance: stop $legacy_unit and run install.sh again to move its database"
            else
                runuser -u ktk-schedule -- sqlite3 "$legacy_db" ".backup '$state/bot.sqlite3'"
                say "$instance: database copied from $legacy_db"
            fi
        fi
    fi
    [ -f "$conf/bot.env" ] || say "$instance: not configured (no $conf/bot.env)"
done

systemctl daemon-reload
for instance in prod test; do
    systemctl enable --quiet --now "ktk-schedule-backup@$instance.timer"
done
say "done"
