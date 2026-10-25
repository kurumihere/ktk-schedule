#!/bin/sh
set -eu

case "${1:-}" in
    restart-test)
        echo "Restarting TEST service (Telegram @test_ktk45_schedule_bot + test VK)..."
        ssh vds "systemctl restart ktk-schedule-test.service"
        sleep 2
        ssh vds "systemctl status ktk-schedule-test.service --lines=5"
        ;;
    restart-prod)
        echo "Restarting PROD service (Telegram @ktk45_schedule_bot)..."
        ssh vds "systemctl restart ktk-schedule.service"
        sleep 2
        ssh vds "systemctl status ktk-schedule.service --lines=5"
        ;;
    restart-all)
        echo "Restarting both services..."
        ssh vds "systemctl restart ktk-schedule.service ktk-schedule-test.service"
        sleep 2
        ssh vds "systemctl status ktk-schedule.service ktk-schedule-test.service --lines=3"
        ;;
    status)
        ssh vds "systemctl status ktk-schedule.service ktk-schedule-test.service --lines=3 || true"
        ;;
    *)
        echo "Usage: $0 {status|restart-test|restart-prod|restart-all}"
        exit 1
        ;;
esac
