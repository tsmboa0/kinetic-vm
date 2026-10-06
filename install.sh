#!/bin/sh
# Install a published KineticVM binary.
#
#   curl -fsSL https://raw.githubusercontent.com/tsmboa0/kinetic-vm/main/install.sh | sh
#
# The binary lands in ~/.local/bin unless KINETIC_INSTALL_DIR is set.
# KINETIC_VERSION pins a tag (default: latest). KINETIC_REPO overrides the
# GitHub repository.

set -eu

repo="${KINETIC_REPO:-tsmboa0/kinetic-vm}"
version="${KINETIC_VERSION:-latest}"

resolve_target() {
    os=$1
    arch=$2
    case "$os" in
        Linux) sys=unknown-linux-gnu ;;
        Darwin) sys=apple-darwin ;;
        *)
            echo "KineticVM installs on Linux and macOS. This machine is ${os}." >&2
            return 1
            ;;
    esac
    case "$arch" in
        x86_64 | amd64) cpu=x86_64 ;;
        aarch64 | arm64) cpu=aarch64 ;;
        armv7* | armv6*)
            echo "KineticVM installs on 64-bit Linux. This machine is reporting ${arch}." >&2
            echo "Raspberry Pi OS needs the 64-bit image." >&2
            return 1
            ;;
        *)
            echo "KineticVM has no published binary for ${arch}." >&2
            return 1
            ;;
    esac
    printf '%s-%s\n' "$cpu" "$sys"
}

if [ "${1:-}" = "--print-target" ]; then
    resolve_target "${KINETIC_OS:-$(uname -s)}" "${KINETIC_ARCH:-$(uname -m)}"
    exit 0
fi

target=$(resolve_target "${KINETIC_OS:-$(uname -s)}" "${KINETIC_ARCH:-$(uname -m)}")
asset="kinetic-${target}.tar.gz"
if [ "$version" = "latest" ]; then
    base="https://github.com/${repo}/releases/latest/download"
else
    base="https://github.com/${repo}/releases/download/${version}"
fi
url="${base}/${asset}"
sums_url="${base}/SHA256SUMS"

if [ -n "${KINETIC_INSTALL_DIR:-}" ]; then
    dest=$KINETIC_INSTALL_DIR
else
    dest="${HOME}/.local/bin"
fi

if [ "${KINETIC_INSTALL_DRY_RUN:-}" = "1" ]; then
    printf 'target=%s\nurl=%s\ndest=%s\n' "$target" "$url" "$dest"
    exit 0
fi

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

echo "Downloading ${asset}"
if ! curl -fsSL --retry 3 -o "${tmpdir}/${asset}" "$url"; then
    echo "Could not download ${url}" >&2
    echo "Release binaries are published from a version tag: https://github.com/${repo}/releases" >&2
    exit 1
fi
if ! curl -fsSL --retry 3 -o "${tmpdir}/SHA256SUMS" "$sums_url"; then
    echo "Could not download the published checksum from ${sums_url}" >&2
    exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "${tmpdir}/${asset}" | awk '{print $1}')
else
    actual=$(shasum -a 256 "${tmpdir}/${asset}" | awk '{print $1}')
fi
expected=$(awk -v name="$asset" '$2 == name { print $1 }' "${tmpdir}/SHA256SUMS")
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "The downloaded archive does not match the published checksum." >&2
    exit 1
fi

entries=$(tar -tzf "${tmpdir}/${asset}")
if [ "$entries" != "kinetic" ]; then
    echo "The release archive does not contain the kinetic binary." >&2
    exit 1
fi
tar -C "$tmpdir" -xzf "${tmpdir}/${asset}"

mkdir -p "$dest"
install -m 755 "${tmpdir}/kinetic" "${dest}/kinetic"

echo "Installed kinetic to ${dest}/kinetic"
case ":${PATH}:" in
    *":${dest}:"*) ;;
    *)
        echo "Add this directory to PATH, then open a new shell:"
        echo "  export PATH=\"${dest}:\$PATH\""
        ;;
esac
echo "Next, run: kinetic quickstart"
