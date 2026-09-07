#!/usr/bin/env bash
#
# Bumps the version, runs the gate, commits and tags — and stops there.
#
# The tag is what `release.yml` reacts to (ADR-049), so pushing it is what
# publishes a release. That push is left to a human on purpose: everything this
# script does is undoable with `git reset` and `git tag -d`, and the push is the
# first step that is not.
#
#   scripts/release.sh patch          # 0.1.0 -> 0.1.1
#   scripts/release.sh minor          # 0.1.0 -> 0.2.0
#   scripts/release.sh major          # 0.1.0 -> 1.0.0
#   scripts/release.sh 0.4.2          # exactly that
#   scripts/release.sh patch --dry-run
#
set -euo pipefail

cd "$(dirname "$0")/.."

die() { echo "error: $*" >&2; exit 1; }

[ $# -ge 1 ] || die "usage: scripts/release.sh <major|minor|patch|X.Y.Z> [--dry-run]"
bump=$1
shift
dry_run=false
for arg in "$@"; do
  case $arg in
    --dry-run) dry_run=true ;;
    *) die "unknown argument: $arg" ;;
  esac
done

# --- preconditions ----------------------------------------------------------
#
# A tag names a commit, so the commit it names has to be one other people can
# fetch. Every check below is about that.

branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" = "main" ] || die "on branch '$branch'; releases are cut from main"

[ -z "$(git status --porcelain)" ] || die "the working tree has changes; commit or stash them first"

git fetch --quiet origin main
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
  die "main and origin/main have diverged; push or pull before releasing"

# --- the version ------------------------------------------------------------
#
# `version` appears once per dependency as well, so the read is scoped to the
# `[package]` table rather than being the first match in the file.

current=$(awk '
  /^\[package\]/  { in_package = 1; next }
  /^\[/           { in_package = 0 }
  in_package && /^version *= *"/ {
    match($0, /"[^"]+"/)
    print substr($0, RSTART + 1, RLENGTH - 2)
    exit
  }
' Cargo.toml)
[ -n "$current" ] || die "no version in the [package] table of Cargo.toml"

case $bump in
  major|minor|patch)
    IFS=. read -r major minor patch <<<"$current"
    case $bump in
      major) major=$((major + 1)); minor=0; patch=0 ;;
      minor) minor=$((minor + 1)); patch=0 ;;
      patch) patch=$((patch + 1)) ;;
    esac
    version="$major.$minor.$patch"
    ;;
  [0-9]*.[0-9]*.[0-9]*)
    version=$bump
    ;;
  *)
    die "'$bump' is neither major, minor, patch, nor an X.Y.Z version"
    ;;
esac

[ "$version" != "$current" ] || die "already at $current; there is nothing to bump"

tag="v$version"
git rev-parse -q --verify "refs/tags/$tag" >/dev/null &&
  die "$tag already exists; a released version is not re-cut"

echo "ferroedit $current -> $version  ($tag)"
if $dry_run; then
  echo "dry run: nothing was changed"
  exit 0
fi

# --- the bump ---------------------------------------------------------------
#
# From here on the tree is dirty, so anything that fails puts it back rather
# than leaving a half-bumped checkout behind.

restore() { git checkout --quiet -- Cargo.toml Cargo.lock 2>/dev/null || true; }
trap restore ERR INT TERM

awk -v version="$version" '
  /^\[package\]/  { in_package = 1 }
  /^\[/ && !/^\[package\]/ { in_package = 0 }
  in_package && /^version *= *"/ && !done {
    print "version = \"" version "\""
    done = 1
    next
  }
  { print }
' Cargo.toml > Cargo.toml.bump && mv Cargo.toml.bump Cargo.toml

# Rewrites the `ferroedit` entry in Cargo.lock, which is the only other place
# the version is written down.
cargo check --quiet --all-features

echo "--- fmt"
cargo fmt --all -- --check
echo "--- clippy"
cargo clippy --all-targets --all-features --quiet -- -D warnings
echo "--- test"
cargo test --all-features --quiet

trap - ERR INT TERM

git add Cargo.toml Cargo.lock
git commit --quiet -m "chore(release): $tag"
git tag -a "$tag" -m "FerroEdit $tag"

cat <<MSG

Committed and tagged $tag. Nothing has been pushed.

  git push origin main $tag

That push is what starts release.yml: it drafts the release, builds the four
targets, attaches their tarballs and checksums, and publishes only once every
one of them is in.

To undo instead:

  git tag -d $tag && git reset --hard HEAD~1
MSG
