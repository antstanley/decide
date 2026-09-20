#!/bin/sh
# Install `decide` from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/antstanley/decide/main/install.sh | sh
#   curl -fsSL https://github.com/antstanley/decide/releases/latest/download/install.sh | sh
#
# Progress goes to stderr, so stdout stays free for a caller that wants it.
#
# Environment:
#   DECIDE_VERSION      a version to install, without the `v` (default: the latest release)
#   DECIDE_INSTALL_DIR  where the binary goes (default: ~/.local/bin, else /usr/local/bin)
#   DECIDE_TARGET       the rustup target triple, overriding what this host looks like — for
#                       fetching another platform's build, or for testing this script
#   DECIDE_BASE_URL     a mirror to download from, laid out as a flat directory
#   DECIDE_REPO         owner/name (default: antstanley/decide)
#
# It is POSIX sh on purpose: `bash 3.2`, which is `/bin/sh` on macOS, cannot parse a `case`
# inside a `$( )`, so every `case` here is at the top level of a function.
set -eu

repo=${DECIDE_REPO:-antstanley/decide}
version=${DECIDE_VERSION:-latest}
install_dir=${DECIDE_INSTALL_DIR:-}
target=${DECIDE_TARGET:-}
base_url=${DECIDE_BASE_URL:-}

say() {
    printf '%s\n' "$*" >&2
}

die() {
    printf 'install: %s\n' "$*" >&2
    exit 1
}

# Progress on stderr, because a `curl | sh` user reads it there and nothing else is on it.
progress() {
    say "  $*"
}

# The rustup target triple this host's build is published under.
detect_target() {
    os=$(uname -s)
    machine=$(uname -m)
    case $os in
        Darwin)
            case $machine in
                arm64 | aarch64) echo aarch64-apple-darwin ;;
                *) die "there is no macOS Intel build; the targets are aarch64-apple-darwin, x86_64-pc-windows-msvc, aarch64-pc-windows-msvc, x86_64-unknown-linux-musl, aarch64-unknown-linux-musl" ;;
            esac
            ;;
        Linux)
            case $machine in
                x86_64 | amd64) echo x86_64-unknown-linux-musl ;;
                aarch64 | arm64) echo aarch64-unknown-linux-musl ;;
                *) die "no Linux build for $machine" ;;
            esac
            ;;
        *)
            die "this installer speaks macOS and Linux; on Windows, download the .zip from https://github.com/$repo/releases/latest"
            ;;
    esac
}

# Where the asset for `$target` lives.
asset_url() {
    if [ -n "$base_url" ]; then
        printf '%s/decide-%s.tar.gz\n' "${base_url%/}" "$target"
        return 0
    fi
    case $version in
        latest) printf 'https://github.com/%s/releases/latest/download/decide-%s.tar.gz\n' "$repo" "$target" ;;
        v*) printf 'https://github.com/%s/releases/download/%s/decide-%s.tar.gz\n' "$repo" "$version" "$target" ;;
        *) printf 'https://github.com/%s/releases/download/v%s/decide-%s.tar.gz\n' "$repo" "$version" "$target" ;;
    esac
}

# The checksum for that asset, or nothing when the release does not carry one.
checksum_url() {
    if [ -n "$base_url" ]; then
        printf '%s/decide-%s.tar.gz.sha256\n' "${base_url%/}" "$target"
        return 0
    fi
    case $version in
        latest) printf 'https://github.com/%s/releases/latest/download/decide-%s.tar.gz.sha256\n' "$repo" "$target" ;;
        v*) printf 'https://github.com/%s/releases/download/%s/decide-%s.tar.gz.sha256\n' "$repo" "$version" "$target" ;;
        *) printf 'https://github.com/%s/releases/download/v%s/decide-%s.tar.gz.sha256\n' "$repo" "$version" "$target" ;;
    esac
}

# A sha256 tool, whichever this host has.
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{ print $1 }'
    else
        die "no sha256sum or shasum to verify the download with"
    fi
}

# The first line of a checksum file, which is either "hash" or "hash  name".
expected_checksum() {
    awk 'NR == 1 { print $1 }' "$1"
}

# ~/.local/bin when it exists or can be made, else /usr/local/bin, else the caller's choice.
choose_install_dir() {
    if [ -n "$install_dir" ]; then
        printf '%s\n' "$install_dir"
        return 0
    fi
    if [ -n "${HOME:-}" ]; then
        printf '%s/.local/bin\n' "$HOME"
        return 0
    fi
    printf '/usr/local/bin\n'
}

install_binary() {
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM

    url=$(asset_url)
    progress "downloading $url"
    curl -fsSL -o "$tmp/decide.tar.gz" "$url" ||
        die "could not download $url"

    # The checksum is the point of the .sha256 assets: an installer that does not verify is a
    # remote code execution endpoint with extra steps.
    if curl -fsSL -o "$tmp/decide.sha256" "$(checksum_url)" 2>/dev/null; then
        want=$(expected_checksum "$tmp/decide.sha256")
        got=$(sha256_of "$tmp/decide.tar.gz")
        [ -n "$want" ] || die "the checksum file is empty"
        [ "$want" = "$got" ] ||
            die "the downloaded archive does not match its checksum (expected $want, got $got)"
        progress "checksum verified"
    else
        die "could not download the checksum for $target; refusing to install unverified bytes"
    fi

    tar -xzf "$tmp/decide.tar.gz" -C "$tmp" || die "the archive could not be unpacked"
    binary=$(find "$tmp" -type f -name decide | head -1)
    [ -n "$binary" ] || die "the archive does not contain a decide binary"

    dir=$(choose_install_dir)
    mkdir -p "$dir" 2>/dev/null || die "cannot create $dir"
    [ -w "$dir" ] || die "$dir is not writable; set DECIDE_INSTALL_DIR to somewhere that is, or re-run with sudo"
    cp "$binary" "$dir/decide" || die "cannot write $dir/decide"
    chmod 755 "$dir/decide"

    installed=$("$dir/decide" --version) || die "$dir/decide does not run on this machine"
    progress "installed $installed to $dir/decide"
    case :$PATH: in
        *:$dir:*) ;;
        *) say "  $dir is not on your PATH; add it with:" ; say "      export PATH=\"$dir:\$PATH\"" ;;
    esac
}

[ -n "$target" ] || target=$(detect_target)
install_binary
