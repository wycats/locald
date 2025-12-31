#!/bin/sh
# locald installer
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/wycats/locald/main/install.sh | sh
#
# Environment variables:
#   LOCALD_VERSION       - Version to install (default: "latest")
#   LOCALD_INSTALL_DIR   - Installation directory (default: "$HOME/.local/bin")
#   LOCALD_NO_MODIFY_PATH - Set to skip PATH modification hint (default: unset)
#
# This script:
#   1. Detects your architecture (x86_64, aarch64)
#   2. Downloads the appropriate binary from GitHub Releases
#   3. Verifies the SHA256 checksum
#   4. Installs to ~/.local/bin (or LOCALD_INSTALL_DIR)
#   5. Prompts you to run `sudo locald admin setup`

set -eu

VERSION="${LOCALD_VERSION:-latest}"
INSTALL_DIR="${LOCALD_INSTALL_DIR:-$HOME/.local/bin}"
REPO="wycats/locald"

main() {
    need_cmd curl
    need_cmd tar
    need_cmd uname
    need_cmd sha256sum

    local _arch
    _arch="$(detect_arch)"
    if [ -z "$_arch" ]; then
        err "Unsupported architecture: $(uname -m). locald supports x86_64 and aarch64."
    fi

    local _os
    _os="$(detect_os)"
    if [ "$_os" != "linux" ]; then
        err "locald currently only supports Linux. macOS support is coming soon.
See: https://github.com/$REPO for updates."
    fi

    local _artifact="locald-linux-${_arch}"
    local _base_url

    if [ "$VERSION" = "latest" ]; then
        _base_url="https://github.com/$REPO/releases/latest/download"
    else
        _base_url="https://github.com/$REPO/releases/download/v${VERSION}"
    fi

    local _tarball_url="${_base_url}/${_artifact}.tar.gz"
    local _checksum_url="${_base_url}/${_artifact}.tar.gz.sha256"

    say "Installing locald for linux-${_arch}..."
    say ""

    # Create temp directory
    local _tmp
    _tmp="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '$_tmp'" EXIT

    # Download tarball
    say "Downloading ${_tarball_url}..."
    ensure curl -fsSL "$_tarball_url" -o "$_tmp/locald.tar.gz"

    # Download and verify checksum
    say "Verifying checksum..."
    ensure curl -fsSL "$_checksum_url" -o "$_tmp/locald.tar.gz.sha256"
    (cd "$_tmp" && sha256sum -c locald.tar.gz.sha256) || err "Checksum verification failed!"

    # Extract
    say "Extracting..."
    ensure tar -xzf "$_tmp/locald.tar.gz" -C "$_tmp"

    # Install
    ensure mkdir -p "$INSTALL_DIR"
    ensure install -m 755 "$_tmp/locald" "$INSTALL_DIR/locald"

    # The shim is included in the tarball but shouldn't be installed by the user directly.
    # It will be installed by `locald admin setup` to the correct location with setuid.

    say ""
    say "✓ Installed locald to $INSTALL_DIR/locald"

    # Check if install dir is in PATH
    if [ -z "${LOCALD_NO_MODIFY_PATH:-}" ]; then
        case ":$PATH:" in
            *":$INSTALL_DIR:"*)
                # Already in PATH
                ;;
            *)
                say ""
                say "Add $INSTALL_DIR to your PATH by adding this to your shell profile:"
                say ""
                say "    export PATH=\"\$PATH:$INSTALL_DIR\""
                say ""
                ;;
        esac
    fi

    # Next steps
    say "────────────────────────────────────────────────────────────────"
    say ""
    say "Next steps:"
    say ""
    say "  1. Complete privileged setup (required once):"
    say ""
    say "       sudo $INSTALL_DIR/locald admin setup"
    say ""
    say "  2. Start using locald in any project:"
    say ""
    say "       cd your-project"
    say "       locald up"
    say ""
    say "For more information: https://github.com/$REPO"
    say ""
}

detect_arch() {
    local _cputype
    _cputype="$(uname -m)"
    case "$_cputype" in
        x86_64 | x86-64 | x64 | amd64)
            echo "x86_64"
            ;;
        aarch64 | arm64)
            echo "aarch64"
            ;;
        *)
            echo ""
            ;;
    esac
}

detect_os() {
    local _ostype
    _ostype="$(uname -s)"
    case "$_ostype" in
        Linux | linux)
            echo "linux"
            ;;
        Darwin | darwin)
            echo "macos"
            ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            echo "windows"
            ;;
        *)
            echo "unknown"
            ;;
    esac
}

say() {
    printf '%s\n' "$*"
}

err() {
    say "Error: $*" >&2
    exit 1
}

need_cmd() {
    if ! command -v "$1" > /dev/null 2>&1; then
        err "Required command '$1' not found. Please install it and try again."
    fi
}

ensure() {
    if ! "$@"; then
        err "Command failed: $*"
    fi
}

main "$@"
