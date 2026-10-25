#!/bin/sh
set -eu

case "${1:-}" in
    test)
        echo "Switching to TEST mode (Telegram @test_ktk45_schedule_bot + VK)..."
        ssh vds "systemctl start ktk-schedule-test.service"
        sleep 2
        ssh vds "systemctl status ktk-schedule-test.service --lines=5"
        ;;
    prod)
        echo "Switching to PROD mode (Telegram @ktk45_schedule_bot + VK)..."
        ssh vds "systemctl start ktk-schedule.service"
        sleep 2
        ssh vds "systemctl status ktk-schedule.service --lines=5"
        ;;
    status)
        ssh vds "systemctl status ktk-schedule.service ktk-schedule-test.service --lines=0 || true"
        ;;
    *)
        echo "Usage: $0 {test|prod|status}"
        exit 1
        ;;
esac
