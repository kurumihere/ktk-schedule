#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
gh workflow run deploy.yml
