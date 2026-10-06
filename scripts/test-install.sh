#!/bin/sh
# Target selection for install.sh. No network.
set -eu

root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
install="${root}/install.sh"

expect_target() {
    os=$1
    arch=$2
    want=$3
    got=$(KINETIC_OS="$os" KINETIC_ARCH="$arch" sh "$install" --print-target)
    if [ "$got" != "$want" ]; then
        echo "expected ${want} for ${os} ${arch}, got ${got}" >&2
        exit 1
    fi
}

expect_fail() {
    os=$1
    arch=$2
    if KINETIC_OS="$os" KINETIC_ARCH="$arch" sh "$install" --print-target >/dev/null 2>&1; then
        echo "expected ${os} ${arch} to be refused" >&2
        exit 1
    fi
}

sh -n "$install"
expect_target Linux aarch64 aarch64-unknown-linux-gnu
expect_target Linux arm64 aarch64-unknown-linux-gnu
expect_target Linux x86_64 x86_64-unknown-linux-gnu
expect_target Linux amd64 x86_64-unknown-linux-gnu
expect_target Darwin arm64 aarch64-apple-darwin
expect_target Darwin x86_64 x86_64-apple-darwin
expect_fail Linux armv7l
expect_fail Windows x86_64

dry=$(KINETIC_OS=Linux KINETIC_ARCH=aarch64 KINETIC_INSTALL_DIR=/tmp/kinetic-bin KINETIC_INSTALL_DRY_RUN=1 sh "$install")
printf '%s\n' "$dry" | grep -q 'target=aarch64-unknown-linux-gnu'
printf '%s\n' "$dry" | grep -q 'url=https://github.com/tsmboa0/kinetic-vm/releases/latest/download/kinetic-aarch64-unknown-linux-gnu.tar.gz'
printf '%s\n' "$dry" | grep -q 'dest=/tmp/kinetic-bin'

echo "install.sh target selection passed"
