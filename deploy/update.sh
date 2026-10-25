#!/bin/sh
set -eu
target="${1:-test}"
cd "$(dirname "$0")/.."
gh workflow run deploy.yml -f target="$target"
