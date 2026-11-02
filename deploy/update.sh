#!/bin/sh
# Build, check and deploy the default branch through GitHub Actions, then follow the run:
#   deploy/update.sh [test|prod]
set -eu

target="${1:-test}"
case "$target" in
    test | prod) ;;
    *)
        echo "usage: $0 [test|prod]" >&2
        exit 64
        ;;
esac
cd "$(dirname "$0")/.."
gh workflow run deploy.yml -f target="$target"
sleep 5
run=$(gh run list --workflow deploy.yml --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$run" --exit-status
