#!/usr/bin/env bash
# Build the Linux packages: target/linux/matrix-finder_<version>-1_<arch>.deb and
# target/linux/matrix-finder-<version>-1.<arch>.rpm (Ubuntu/Debian, Fedora/RHEL).
#
#   scripts/linux/build-packages.sh               # this machine's architecture
#   scripts/linux/build-packages.sh --arch amd64  # or arm64 (another one is emulated: slow)
#
# Both are built from one binary, compiled in Docker on Ubuntu 22.04 (glibc 2.35,
# OpenSSL 3), so they run on Ubuntu 22.04+, Debian 12+ and current Fedora. Needs Docker;
# the source folder is mounted read-only. Rust, cargo-deb and cargo-generate-rpm are
# kept in Docker volumes named matrix-finder-pkg-*-<arch> (remove them with
# `docker volume rm`).
#
# The packages install the same files as scripts/install.sh, under /usr. What the
# package metadata says (dependencies, files) is in Cargo.toml under
# [package.metadata.deb] and [package.metadata.generate-rpm].
set -euo pipefail

die() { echo "error: $*" >&2; exit 1; }

arch=""
while [ $# -gt 0 ]; do
    case "$1" in
        --arch) [ $# -ge 2 ] || die "--arch needs amd64 or arm64"; arch=$2; shift ;;
        -h|--help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
    shift
done
if [ -z "$arch" ]; then
    case "$(uname -m)" in
        x86_64|amd64) arch=amd64 ;;
        arm64|aarch64) arch=arm64 ;;
        *) die "unknown architecture $(uname -m); pass --arch amd64 or --arch arm64" ;;
    esac
fi
case "$arch" in amd64|arm64) ;; *) die "--arch is amd64 or arm64" ;; esac

command -v docker >/dev/null || die "Docker is needed"
docker info >/dev/null 2>&1 || die "Docker is not running"

root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/target/linux"
mkdir -p "$out"
vol="matrix-finder-pkg"

docker run --rm --platform "linux/$arch" \
    -v "$root":/src:ro \
    -v "$out":/out \
    -v "$vol-cargo-$arch":/root/.cargo \
    -v "$vol-rustup-$arch":/root/.rustup \
    -v "$vol-target-$arch":/target \
    -e CARGO_TARGET_DIR=/target \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    -w /src ubuntu:22.04 bash -euo pipefail -c '
export DEBIAN_FRONTEND=noninteractive PATH=/root/.cargo/bin:$PATH
echo "==> System packages"
apt-get update -qq >/dev/null
apt-get install -y -qq build-essential pkg-config libssl-dev ca-certificates curl >/dev/null
if ! command -v cargo >/dev/null; then
    echo "==> Rust"
    curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal >/dev/null
else
    # The toolchain kept in the volume may be older than Cargo.toml's rust-version.
    rustup update --no-self-update stable >/dev/null
fi
command -v cargo-deb >/dev/null || { echo "==> cargo-deb"; cargo install --locked cargo-deb; }
command -v cargo-generate-rpm >/dev/null || { echo "==> cargo-generate-rpm"; cargo install --locked cargo-generate-rpm; }
echo "==> Building"
cargo build --release --locked
echo "==> Packaging"
cargo deb --no-build --no-strip --output /out/
cargo generate-rpm --target-dir /target --output /out/
chown "$HOST_UID:$HOST_GID" /out/*.deb /out/*.rpm
'
echo "==> Packages in $out"
ls -1 "$out"/*.deb "$out"/*.rpm
