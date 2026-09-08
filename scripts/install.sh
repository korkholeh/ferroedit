#!/bin/sh
#
# Installs a FerroEdit release binary.
#
#   curl -fsSL https://raw.githubusercontent.com/korkholeh/ferroedit/main/scripts/install.sh | sh
#
# It picks the build for this machine, downloads it from the GitHub Release
# (ADR-049), checks it against the `.sha256` published beside it, and puts the
# binary in ~/.local/bin. Nothing else is touched: no package manager, no shell
# profile, no sudo.
#
#   sh install.sh --version v0.1.1     # a specific release instead of the latest
#   sh install.sh --dir /usr/local/bin # somewhere else
#   sh install.sh --help
#
# Through a pipe the arguments go after `-s --`:
#
#   curl -fsSL <url> | sh -s -- --version v0.1.1
#
# POSIX sh on purpose. `curl | sh` runs whatever /bin/sh is, which on Debian and
# Ubuntu is dash, so there are no arrays, no [[ ]] and no <<< in here.
#
# Everything is inside main(), called on the last line, so a download cut off
# halfway executes nothing rather than half of an installer (ADR-060).

set -eu

REPO=korkholeh/ferroedit
BIN=ferroedit

say()  { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

usage() {
  cat <<USAGE
Install FerroEdit.

  --version <tag>   the release to install, e.g. v0.1.1 (default: the latest)
  --dir <path>      where to put the binary (default: \$HOME/.local/bin)
  --help            this

Environment: FERROEDIT_VERSION, FERROEDIT_INSTALL_DIR, FERROEDIT_NO_VERIFY=1
USAGE
}

# --- the machine ------------------------------------------------------------
#
# The four names here are exactly the four targets release.yml builds; anything
# else has no binary to download and has to say so rather than guess.

detect_target() {
  arch=$(uname -m)
  case $arch in
    x86_64 | amd64) arch=x86_64 ;;
    arm64 | aarch64) arch=aarch64 ;;
    *) die "no FerroEdit build for the $arch architecture; build from source instead" ;;
  esac

  os=$(uname -s)
  case $os in
    Linux) target="$arch-unknown-linux-musl" ;;
    Darwin)
      # Under Rosetta `uname -m` answers for the translated process, not for the
      # Mac, so an Apple silicon machine would be handed the Intel build and run
      # it emulated forever.
      if [ "$arch" = x86_64 ] &&
         [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
        arch=aarch64
      fi
      target="$arch-apple-darwin"
      ;;
    *) die "no FerroEdit build for $os; Windows is not part of the MVP" ;;
  esac
}

# --- fetching ---------------------------------------------------------------

detect_fetcher() {
  if command -v curl >/dev/null 2>&1; then
    fetcher=curl
  elif command -v wget >/dev/null 2>&1; then
    fetcher=wget
  else
    die "neither curl nor wget is installed"
  fi
}

fetch() { # url dest
  case $fetcher in
    curl) curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1" ;;
    wget) wget -q -O "$2" "$1" ;;
  esac || die "could not download $1"
}

# The latest release is read from the redirect on /releases/latest rather than
# from the API: api.github.com allows sixty unauthenticated calls an hour per
# address, which a shared NAT or a CI runner can have spent already, and an
# installer must not fail on someone else's rate limit. wget cannot report a
# redirect target usefully, so it falls back to the API.
latest_tag() {
  case $fetcher in
    curl)
      url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
        "https://github.com/$REPO/releases/latest") || url=
      tag=${url##*/}
      ;;
    wget)
      tag=$(wget -qO- "https://api.github.com/repos/$REPO/releases/latest" |
        sed -n 's/.*"tag_name" *: *"\([^"]*\)".*/\1/p' | head -n 1)
      ;;
  esac
  case $tag in
    v*) printf '%s\n' "$tag" ;;
    *) die "could not work out the latest release; pass --version <tag>" ;;
  esac
}

# --- the checksum -----------------------------------------------------------
#
# macOS ships `shasum`, Linux ships `sha256sum`, and a machine with neither is
# rare enough that refusing is better than quietly installing an unverified
# binary. FERROEDIT_NO_VERIFY=1 is the way out for the minimal container where
# that is genuinely the situation.

sha256_of() { # file
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    return 1
  fi
}

verify() { # tarball sumfile
  if [ "${FERROEDIT_NO_VERIFY:-0}" = 1 ]; then
    warn "FERROEDIT_NO_VERIFY=1, so the download was not checked"
    return 0
  fi
  got=$(sha256_of "$1") ||
    die "no sha256sum or shasum to check the download with; set FERROEDIT_NO_VERIFY=1 to install anyway"
  # The published file is `<hash>  <filename>`; only the hash is ours to compare.
  want=$(cut -d' ' -f1 <"$2")
  [ -n "$want" ] || die "the published checksum is empty"
  [ "$got" = "$want" ] ||
    die "checksum mismatch: got $got, expected $want. The download is corrupt or tampered with; nothing was installed."
}

main() {
  version=${FERROEDIT_VERSION:-}
  dir=${FERROEDIT_INSTALL_DIR:-$HOME/.local/bin}

  while [ $# -gt 0 ]; do
    case $1 in
      --version) [ $# -ge 2 ] || die "--version needs a tag"; version=$2; shift 2 ;;
      --dir) [ $# -ge 2 ] || die "--dir needs a path"; dir=$2; shift 2 ;;
      -h | --help) usage; exit 0 ;;
      *) die "unknown argument: $1" ;;
    esac
  done

  detect_fetcher
  detect_target

  if [ -z "$version" ]; then
    version=$(latest_tag)
  else
    # `--version 0.1.1` is the obvious slip, and the tags are all `v`-prefixed.
    case $version in v*) ;; *) version="v$version" ;; esac
  fi

  asset="$BIN-$target.tar.gz"
  base="https://github.com/$REPO/releases/download/$version"

  say "FerroEdit $version ($target) -> $dir"

  tmp=$(mktemp -d "${TMPDIR:-/tmp}/ferroedit-install.XXXXXX") ||
    die "could not make a temporary directory"
  # Whatever happens next — a failed download, a Ctrl-C mid-extract — the
  # half-unpacked tarball does not outlive the run.
  trap 'rm -rf "$tmp"' EXIT INT TERM

  fetch "$base/$asset" "$tmp/$asset"
  fetch "$base/$asset.sha256" "$tmp/$asset.sha256"
  verify "$tmp/$asset" "$tmp/$asset.sha256"

  tar -xzf "$tmp/$asset" -C "$tmp" || die "could not unpack $asset"
  [ -f "$tmp/$BIN" ] || die "$asset did not contain a $BIN binary"
  chmod +x "$tmp/$BIN"

  mkdir -p "$dir" || die "could not create $dir"
  [ -w "$dir" ] || die "$dir is not writable; pass --dir somewhere it is, or create it first"

  # Copied next to the destination and then renamed, rather than copied over it.
  # rename(2) is atomic and unlinks the old inode, so upgrading while an editor
  # is open works instead of failing with ETXTBSY, and an interrupted copy never
  # leaves a truncated `ferroedit` on PATH.
  staged="$dir/.$BIN.install.$$"
  cp "$tmp/$BIN" "$staged" || die "could not write to $dir"
  chmod 755 "$staged"
  mv -f "$staged" "$dir/$BIN" || { rm -f "$staged"; die "could not install into $dir"; }

  installed=$("$dir/$BIN" --version 2>/dev/null) || installed="$BIN $version"
  say "Installed $installed"

  case ":${PATH:-}:" in
    *":$dir:"*) say "Run: $BIN ." ;;
    *)
      say ""
      warn "$dir is not on your PATH."
      say "  Add it, then start a new shell:"
      say ""
      say "    echo 'export PATH=\"$dir:\$PATH\"' >> ~/.profile"
      say ""
      say "  Until then: $dir/$BIN ."
      ;;
  esac
}

main "$@"
