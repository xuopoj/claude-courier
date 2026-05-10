#!/bin/sh
# claude-courier installer
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/xuopoj/claude-courier/main/install.sh | sh
#
# Environment overrides:
#   VERSION=v0.1.0                           # install a specific tag (default: latest release)
#   PREFIX=/usr/local/bin                    # install dir (default: /usr/local/bin, falls back to ~/.local/bin without sudo)
#   NO_SUDO=1                                # skip sudo, install to ~/.local/bin
#   GITHUB_REPO=xuopoj/claude-courier        # source repo (rarely needed)
set -eu

GITHUB_REPO="${GITHUB_REPO:-xuopoj/claude-courier}"
BIN_NAME="claude-courier"

err() { printf '%s\n' "error: $*" >&2; exit 1; }
say() { printf '%s\n' "$*" >&2; }

# Detect platform → target triple. Mirrors the matrix in .github/workflows/release.yml.
detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os/$arch" in
        Darwin/arm64)         echo "aarch64-apple-darwin" ;;
        Linux/x86_64|Linux/amd64) echo "x86_64-unknown-linux-gnu" ;;
        Linux/aarch64|Linux/arm64) echo "aarch64-unknown-linux-gnu" ;;
        *) err "unsupported platform: $os/$arch (open an issue at https://github.com/$GITHUB_REPO)" ;;
    esac
}

# Resolve the target version. Default = latest release.
resolve_version() {
    if [ -n "${VERSION:-}" ]; then
        echo "$VERSION"
        return
    fi
    # Use the GitHub API redirect: /releases/latest -> /releases/tag/<TAG>
    url=$(curl -fsSL -o /dev/null -w '%{url_effective}' \
              "https://github.com/$GITHUB_REPO/releases/latest") \
        || err "could not resolve latest release (network issue or no releases yet)"
    case "$url" in
        */tag/*) echo "${url##*/tag/}" ;;
        *) err "unexpected redirect target: $url" ;;
    esac
}

# Pick install destination. Honors PREFIX, NO_SUDO; otherwise tries /usr/local/bin via sudo, falls back to ~/.local/bin.
pick_prefix() {
    if [ -n "${PREFIX:-}" ]; then
        echo "$PREFIX"
        return
    fi
    if [ "${NO_SUDO:-0}" = "1" ]; then
        mkdir -p "$HOME/.local/bin"
        echo "$HOME/.local/bin"
        return
    fi
    if [ -w /usr/local/bin ] 2>/dev/null; then
        echo "/usr/local/bin"
        return
    fi
    if command -v sudo >/dev/null 2>&1; then
        echo "/usr/local/bin"
        return
    fi
    mkdir -p "$HOME/.local/bin"
    echo "$HOME/.local/bin"
}

target=$(detect_target)
version=$(resolve_version)
prefix=$(pick_prefix)
strip_v="${version#v}"
tarball="${BIN_NAME}-${strip_v}-${target}.tar.gz"
url="https://github.com/$GITHUB_REPO/releases/download/${version}/${tarball}"
sha_url="${url}.sha256"

say "platform : $target"
say "version  : $version"
say "prefix   : $prefix"
say "tarball  : $tarball"

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT

say "downloading..."
curl -fsSL -o "$tmpdir/$tarball" "$url" \
    || err "could not download $url"
curl -fsSL -o "$tmpdir/$tarball.sha256" "$sha_url" \
    || err "could not download $sha_url (checksum file missing for this release)"

say "verifying sha256..."
expected=$(awk '{print $1}' "$tmpdir/$tarball.sha256")
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmpdir/$tarball" | awk '{print $1}')
elif command -v shasum >/dev/null 2>&1; then
    actual=$(shasum -a 256 "$tmpdir/$tarball" | awk '{print $1}')
else
    err "no sha256sum/shasum available — refusing to install unverified binary"
fi
[ "$expected" = "$actual" ] || err "sha256 mismatch (expected $expected, got $actual)"

say "extracting..."
tar -xzf "$tmpdir/$tarball" -C "$tmpdir"
src="$tmpdir/${BIN_NAME}-${strip_v}-${target}/${BIN_NAME}"
[ -x "$src" ] || err "binary not found in tarball at $src"

say "installing to $prefix/$BIN_NAME ..."
case "$prefix" in
    /usr/local/bin|/usr/bin|/opt/*)
        if [ -w "$prefix" ]; then
            install -m 0755 "$src" "$prefix/$BIN_NAME"
        else
            sudo install -m 0755 "$src" "$prefix/$BIN_NAME"
        fi
        ;;
    *)
        install -m 0755 "$src" "$prefix/$BIN_NAME"
        ;;
esac

if ! command -v "$BIN_NAME" >/dev/null 2>&1; then
    say ""
    say "warning: $prefix is not on your PATH."
    say "Add this to your shell rc:"
    say "    export PATH=\"$prefix:\$PATH\""
fi

say ""
say "installed: $("$prefix/$BIN_NAME" --version 2>/dev/null || echo "$prefix/$BIN_NAME")"
say "next steps:"
say "  $BIN_NAME publish-configure --broker https://broker.aishipbox.com --key <PUBLISH_KEY>   # publisher (Mac)"
say "  $BIN_NAME consume-configure --broker https://broker.aishipbox.com --key <CONSUME_KEY>   # consumer"
