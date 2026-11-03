#!/bin/sh
# Build on this machine and deploy over SSH:
#   deploy/update.sh [test|prod] [--quick]
#
#   1. fmt, clippy and tests (--quick skips them; prod never does)
#   2. release build; a binary that needs a newer glibc than the server has is refused
#   3. the binary goes compressed over SSH to ktk-schedule-deploy, which rehearses the
#      migration on a copy of the live database, backs it up, swaps the binary, waits for
#      READY and 15 s of stability, and restores binary and database on any failure
#
# prod deploys only a clean, committed tree. The SSH host alias defaults to "vps"
# (override with KTK_SSH_HOST); it must log in as root.
set -eu

target=test
quick=no
usage() {
    echo "usage: $0 [test|prod] [--quick]" >&2
    exit 64
}
for arg in "$@"; do
    case "$arg" in
        test | prod) target=$arg ;;
        --quick) quick=yes ;;
        *) usage ;;
    esac
done
host="${KTK_SSH_HOST:-vps}"
say() { printf 'update %s: %s\n' "$target" "$*" >&2; }
die() {
    say "FAILED: $*"
    exit 1
}

cd "$(dirname "$0")/.."
commit=$(git rev-parse --short HEAD)
if [ -n "$(git status --porcelain)" ]; then
    [ "$target" = test ] || die "prod deploys a commit: commit or stash the changes first"
    commit="$commit+dirty"
fi
[ "$target" = prod ] && quick=no

start=$(date +%s)
if [ "$quick" = no ]; then
    say "checks"
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked --all-targets
fi
say "release build"
cargo build --locked --release
bin=target/release/ktk-schedule
version=$("$bin" --version)

# The server runs Debian's glibc; the build machine may have a newer one.
need=$(objdump -T "$bin" | grep -oE 'GLIBC_[0-9]+\.[0-9]+(\.[0-9]+)?' | sed 's/^GLIBC_//' | sort -uV | tail -n 1)
have=$(ssh "$host" getconf GNU_LIBC_VERSION | awk '{print $2}')
[ -n "$need" ] && [ -n "$have" ] || die "could not compare glibc versions"
[ "$(printf '%s\n%s\n' "$need" "$have" | sort -V | tail -n 1)" = "$have" ] ||
    die "the binary needs glibc $need, $host has $have"

sha=$(sha256sum "$bin" | cut -d' ' -f1)
say "shipping $version ($commit, sha256 $sha, glibc $need <= $have)"
# zstd checks the content checksum; the server checks the size and that the binary runs.
zstd -q -c -T0 "$bin" |
    ssh "$host" "zstd -q -d -c | /usr/local/sbin/ktk-schedule-deploy $target" ||
    die "deploy failed (the server rolled back if it got that far)"
say "done in $(($(date +%s) - start)) s"
