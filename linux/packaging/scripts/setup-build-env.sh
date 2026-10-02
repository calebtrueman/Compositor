#!/usr/bin/env bash
# Prepares a Debian/Ubuntu container (the release baseline is ubuntu:20.04, glibc 2.31) to
# build and package Compositor: system packages, a Rust toolchain via rustup, cargo-deb and
# cargo-generate-rpm. Run as root. Used by .github/workflows/linux-release.yml and for local
# builds (see linux/packaging/README.md).
set -euo pipefail

RUST_TOOLCHAIN="${RUST_TOOLCHAIN:-stable}"
CARGO_DEB_VERSION="${CARGO_DEB_VERSION:-3.8.0}"
CARGO_GENERATE_RPM_VERSION="${CARGO_GENERATE_RPM_VERSION:-0.21.0}"

export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y --no-install-recommends \
  ca-certificates curl git xz-utils zstd file binutils build-essential pkg-config \
  dpkg-dev desktop-file-utils appstream \
  libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev \
  libgl1-mesa-dev libegl1-mesa-dev

if ! command -v rustup >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --profile minimal --default-toolchain "$RUST_TOOLCHAIN"
fi
# shellcheck disable=SC1091
source "${CARGO_HOME:-$HOME/.cargo}/env"
rustc --version

if [ "${SKIP_PACKAGING_TOOLS:-0}" != 1 ]; then
  cargo install --locked --version "$CARGO_DEB_VERSION" cargo-deb
  cargo install --locked --version "$CARGO_GENERATE_RPM_VERSION" cargo-generate-rpm
fi
