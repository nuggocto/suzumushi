#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

readonly VERSION="${1:-}"
readonly TARGET="${2:-}"
readonly COMMIT="${3:-}"
readonly OUTPUT_DIR="${4:-}"

if [[ "$VERSION" != [0-9]* || "$VERSION" == *[!0-9A-Za-z.+-]* ]]; then
  echo "error: invalid Cargo version: $VERSION" >&2
  exit 2
fi

if [[ "$TARGET" != "x86_64-unknown-linux-gnu" ]]; then
  echo "error: unsupported release target: $TARGET" >&2
  exit 2
fi

if [[ ! "$COMMIT" =~ ^[0-9a-f]{40}$ ]] || ! git cat-file -e "$COMMIT^{commit}"; then
  echo "error: release commit must be a full local Git commit ID" >&2
  exit 2
fi

if [[ -z "$OUTPUT_DIR" ]]; then
  echo "usage: $0 <version> <target> <commit> <output-directory>" >&2
  exit 2
fi

readonly BINARY="target/$TARGET/release/suzumushi"
readonly ARCHIVE_ROOT="suzumushi-v$VERSION-$TARGET"
readonly ARCHIVE="$ARCHIVE_ROOT.tar.xz"
readonly COMMIT_TIME="$(git show -s --format=%ct "$COMMIT")"

if [[ ! -x "$BINARY" ]]; then
  echo "error: release binary does not exist: $BINARY" >&2
  exit 1
fi

if [[ "$($BINARY --version)" != "suzumushi $VERSION" ]]; then
  echo "error: release binary version does not match $VERSION" >&2
  exit 1
fi

mkdir -p -- "$OUTPUT_DIR"
if [[ -e "$OUTPUT_DIR/$ARCHIVE" || -e "$OUTPUT_DIR/SHA256SUMS" ]]; then
  echo "error: refusing to overwrite existing release output" >&2
  exit 1
fi

package_dir="$(mktemp -d "${TMPDIR:-/tmp}/suzumushi-release.XXXXXX")"
cleanup() {
  rm -rf -- "${package_dir:?}"
}
trap cleanup EXIT HUP INT TERM

install -Dm755 "$BINARY" "$package_dir/$ARCHIVE_ROOT/suzumushi"
install -Dm644 README.md "$package_dir/$ARCHIVE_ROOT/README.md"
install -Dm644 CHANGELOG.md "$package_dir/$ARCHIVE_ROOT/CHANGELOG.md"
install -Dm644 LICENSE "$package_dir/$ARCHIVE_ROOT/LICENSE"
ln -s suzumushi "$package_dir/$ARCHIVE_ROOT/suzu"

LC_ALL=C tar \
  --create \
  --directory "$package_dir" \
  --sort=name \
  --mtime="@$COMMIT_TIME" \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --format=ustar \
  "$ARCHIVE_ROOT" \
  | xz --threads=1 --check=crc64 -9 >"$OUTPUT_DIR/$ARCHIVE"

(
  cd "$OUTPUT_DIR"
  sha256sum "$ARCHIVE" >SHA256SUMS
)

scripts/verify-release-archive.sh \
  "$VERSION" \
  "$TARGET" \
  "$OUTPUT_DIR/$ARCHIVE" \
  "$OUTPUT_DIR/SHA256SUMS"
