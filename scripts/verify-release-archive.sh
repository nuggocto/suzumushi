#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0

set -euo pipefail

readonly VERSION="${1:-}"
readonly TARGET="${2:-}"
readonly ARCHIVE_PATH="${3:-}"
readonly CHECKSUM_PATH="${4:-}"

if [[ "$VERSION" != [0-9]* || "$VERSION" == *[!0-9A-Za-z.+-]* ]]; then
  echo "error: invalid Cargo version: $VERSION" >&2
  exit 2
fi

if [[ "$TARGET" != "x86_64-unknown-linux-gnu" ]]; then
  echo "error: unsupported release target: $TARGET" >&2
  exit 2
fi

if [[ ! -f "$ARCHIVE_PATH" || ! -f "$CHECKSUM_PATH" ]]; then
  echo "error: archive and checksum files are required" >&2
  exit 2
fi

readonly ARCHIVE_ROOT="suzumushi-v$VERSION-$TARGET"
readonly ARCHIVE_NAME="$ARCHIVE_ROOT.tar.xz"
readonly ARCHIVE_DIR="$(cd "$(dirname -- "$ARCHIVE_PATH")" && pwd -P)"

if [[ "$(basename -- "$ARCHIVE_PATH")" != "$ARCHIVE_NAME" ]]; then
  echo "error: unexpected archive name: $(basename -- "$ARCHIVE_PATH")" >&2
  exit 1
fi

readonly EXPECTED_SUM="$(cd "$ARCHIVE_DIR" && sha256sum "$ARCHIVE_NAME")"
readonly RECORDED_SUM="$(<"$CHECKSUM_PATH")"
if [[ "$RECORDED_SUM" != "$EXPECTED_SUM" ]]; then
  echo "error: SHA256SUMS does not exactly match $ARCHIVE_NAME" >&2
  exit 1
fi

readonly EXPECTED_CONTENTS="$ARCHIVE_ROOT/
$ARCHIVE_ROOT/CHANGELOG.md
$ARCHIVE_ROOT/LICENSE
$ARCHIVE_ROOT/README.md
$ARCHIVE_ROOT/suzu
$ARCHIVE_ROOT/suzumushi"
readonly ACTUAL_CONTENTS="$(LC_ALL=C tar --list --file "$ARCHIVE_PATH" | LC_ALL=C sort)"
if [[ "$ACTUAL_CONTENTS" != "$EXPECTED_CONTENTS" ]]; then
  echo "error: release archive contains unexpected paths" >&2
  diff -u <(printf '%s\n' "$EXPECTED_CONTENTS") <(printf '%s\n' "$ACTUAL_CONTENTS") >&2 || true
  exit 1
fi

verify_dir="$(mktemp -d "${TMPDIR:-/tmp}/suzumushi-verify.XXXXXX")"
cleanup() {
  rm -rf -- "${verify_dir:?}"
}
trap cleanup EXIT HUP INT TERM

tar --extract --file "$ARCHIVE_PATH" --directory "$verify_dir"

readonly EXTRACTED="$verify_dir/$ARCHIVE_ROOT"
if [[ ! -x "$EXTRACTED/suzumushi" ]]; then
  echo "error: canonical release binary is not executable" >&2
  exit 1
fi

if [[ ! -L "$EXTRACTED/suzu" || "$(readlink "$EXTRACTED/suzu")" != "suzumushi" ]]; then
  echo "error: suzu must be a relative symlink to suzumushi" >&2
  exit 1
fi

if [[ "$($EXTRACTED/suzumushi --version)" != "suzumushi $VERSION" ]]; then
  echo "error: extracted binary version does not match $VERSION" >&2
  exit 1
fi
