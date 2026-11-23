#!/usr/bin/env bash
# Cut a Cadence release: bump the version, run every check CI runs before
# packaging, commit "X.Y.Z: <message>", and print (or, with --push, run)
# the commands that publish it. Pushing the vX.Y.Z tag makes CI build, sign
# and publish the release (see README, Releasing).
#
# Usage:  scripts/release.sh [--push] [--notes FILE] X.Y.Z "what changed"
#
#   --push        push main and the tag right away instead of printing the
#                 commands
#   --notes FILE  extra release notes (plain text / short markdown) for the
#                 tag message; CI puts the tag message into latest.json as
#                 the in-app "what's new" text (capped at 4 KB)
#
# Requirements: on the main branch, a clean tree, and up to date with
# origin/main. Nothing is committed if a check fails; the version bump is
# undone. CARGO overrides the cargo program (for testing this script).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
export PATH="$HOME/.cargo/bin:$PATH"
CARGO="${CARGO:-cargo}"

usage() {
  sed -n '7,14p' "$0" | sed 's/^# \{0,1\}//' >&2
  exit 2
}
die() {
  echo "error: $*" >&2
  exit 1
}

PUSH=0
NOTES_FILE=""
ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --push) PUSH=1 ;;
    --notes)
      [ $# -ge 2 ] || usage
      NOTES_FILE="$2"
      shift
      ;;
    -h | --help) usage ;;
    -*) echo "unknown option $1" >&2; usage ;;
    *) ARGS+=("$1") ;;
  esac
  shift
done
[ "${#ARGS[@]}" -eq 2 ] || usage
VERSION="${ARGS[0]#v}"
MESSAGE="${ARGS[1]}"
TAG="v$VERSION"

echo "$VERSION" | grep -Eq '^(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})$' ||
  die "version must be X.Y.Z (got \"${ARGS[0]}\")"
[ -n "${MESSAGE//[[:space:]]/}" ] || die "the release message is empty"
case "$MESSAGE" in
  *$'\n'*) die "the release message must be one line (use --notes FILE for more)" ;;
esac
if [ -n "$NOTES_FILE" ]; then
  [ -f "$NOTES_FILE" ] || die "notes file $NOTES_FILE not found"
  NOTES_FILE="$(cd "$(dirname "$NOTES_FILE")" && pwd)/$(basename "$NOTES_FILE")"
fi

# ----- the repository -------------------------------------------------------
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = main ] || die "releases are cut from main (this is $BRANCH)"
[ -z "$(git status --porcelain)" ] || die "the working tree has uncommitted changes"
git fetch --quiet origin main || die "could not fetch origin/main"
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] ||
  die "main is not the same as origin/main (pull or push first)"
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  die "tag $TAG already exists locally"
fi
if git ls-remote --exit-code --tags origin "refs/tags/$TAG" >/dev/null 2>&1; then
  die "tag $TAG already exists on origin"
fi

CURRENT="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)"
[ -n "$CURRENT" ] || die "could not read the version from Cargo.toml"
newer() { # is $1 > $2 ?
  [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)" = "$1" ]
}
newer "$VERSION" "$CURRENT" || die "$VERSION is not newer than the current version $CURRENT"

# ----- bump + checks ----------------------------------------------------------
restore() {
  echo "Restoring Cargo.toml and Cargo.lock." >&2
  git checkout --quiet -- Cargo.toml Cargo.lock
}
trap restore ERR

echo "==> Bumping $CURRENT -> $VERSION"
# Only the [package] version: the first `version = ` line of Cargo.toml.
awk -v v="$VERSION" '!done && /^version = "/ { print "version = \"" v "\""; done = 1; next } { print }' \
  Cargo.toml > Cargo.toml.release-tmp
mv Cargo.toml.release-tmp Cargo.toml
"$CARGO" update -p cadence-studio --offline
grep -q "^version = \"$VERSION\"" Cargo.toml || { restore; die "the version bump did not apply"; }

echo "==> cargo fmt --all -- --check"
"$CARGO" fmt --all -- --check
echo "==> cargo clippy --locked --all-targets -- -D warnings"
"$CARGO" clippy --locked --all-targets -- -D warnings
echo "==> cargo test --locked --all-targets"
"$CARGO" test --locked --all-targets
trap - ERR

# ----- commit + tag message -------------------------------------------------------
git add Cargo.toml Cargo.lock
git commit --quiet -m "$VERSION: $MESSAGE"
echo "==> Committed $(git log -1 --format='%h %s')"

TAG_MSG="$(mktemp "${TMPDIR:-/tmp}/cadence-tag.XXXXXX")"
trap 'rm -f "$TAG_MSG"' EXIT
{
  printf '%s\n' "$MESSAGE"
  if [ -n "$NOTES_FILE" ]; then
    printf '\n'
    cat "$NOTES_FILE"
  fi
} > "$TAG_MSG"

if [ "$PUSH" = 1 ]; then
  echo "==> Pushing main and $TAG"
  git push origin HEAD:main
  git tag -a "$TAG" -F "$TAG_MSG"
  git push origin "$TAG"
  echo "Done: CI now builds, signs and publishes Cadence $VERSION."
else
  quoted="$(printf '%q' "$MESSAGE")"
  echo
  echo "Ready. To publish Cadence $VERSION run:"
  echo
  if [ -n "$NOTES_FILE" ]; then
    notes_q="$(printf '%q' "$NOTES_FILE")"
    echo "  git push origin HEAD:main && git tag -a $TAG -m $quoted -m \"\$(cat $notes_q)\" && git push origin $TAG"
  else
    echo "  git push origin HEAD:main && git tag -a $TAG -m $quoted && git push origin $TAG"
  fi
  echo
  echo "(or undo with: git reset --hard HEAD~1)"
fi
