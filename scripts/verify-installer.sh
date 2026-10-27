#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Exercises a rendered install.sh against the local release archive. The copy
# under test reads the archive through file:// instead of the GitHub release.

set -euo pipefail

readonly VERSION="${1:-}"
readonly DIST_DIR="${2:-}"

if [[ -z "$VERSION" || ! -d "$DIST_DIR" ]]; then
  echo "usage: $0 <version> <dist-directory>" >&2
  exit 2
fi

DIST="$(cd -- "$DIST_DIR" && pwd -P)"
readonly DIST
readonly SCRIPT="$DIST/install.sh"
readonly ARCHIVE="$DIST/suzumushi-v$VERSION-x86_64-unknown-linux-gnu.tar.xz"
test -f "$SCRIPT"
test -f "$ARCHIVE"

qa_dir="$(mktemp -d "${TMPDIR:-/tmp}/suzumushi-installer-qa.XXXXXX")"
cleanup() {
  rm -rf -- "${qa_dir:?}"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

fail() {
  echo "error: $1" >&2
  exit 1
}

# Serve the archive from disk; every other line stays as published.
local_copy() {
  sed \
    -e "s|^base_url=.*|base_url='file://$DIST'|" \
    -e "s|--proto '=https' --proto-redir '=https' --tlsv1.2|--proto '=file'|" \
    "$1"
}
local_copy "$SCRIPT" >"$qa_dir/install.sh"
grep -q "file://$DIST" "$qa_dir/install.sh" || fail 'could not redirect the installer to the local archive'
grep -q "proto '=file'" "$qa_dir/install.sh" || fail 'could not relax the protocol for the local archive'

bin="$qa_dir/bin with spaces"
run_installer() {
  HOME="$qa_dir/home" TMPDIR="$qa_dir" sh "$qa_dir/install.sh" --bin-dir "$bin"
}
check_installed() {
  [[ "$("$bin/suzumushi" --version)" == "suzumushi $VERSION" ]] || fail 'installed version differs'
  [[ -L "$bin/suzu" && "$(readlink "$bin/suzu")" == suzumushi ]] || fail 'suzu is not a relative link'
  [[ "$("$bin/suzu" --version)" == "suzumushi $VERSION" ]] || fail 'suzu does not launch suzumushi'
}

# Truncated downloads must run nothing: half the script, and all but the last line.
size="$(wc -c <"$qa_dir/install.sh")"
last="$(grep -bo '^main "\$@"' "$qa_dir/install.sh" | cut -d: -f1)"
for cut in $((size / 2)) "$last"; do
  head -c "$cut" "$qa_dir/install.sh" | HOME="$qa_dir/home" TMPDIR="$qa_dir" sh -s -- --bin-dir "$bin" || true
  [[ ! -e "$bin" ]] || fail "a script truncated at byte $cut installed files"
done

# A complete piped install, then a reinstall over it.
HOME="$qa_dir/home" TMPDIR="$qa_dir" sh -s -- --bin-dir "$bin" <"$qa_dir/install.sh"
check_installed
before="$(sha256sum <"$bin/suzumushi")"
run_installer
check_installed
[[ "$(sha256sum <"$bin/suzumushi")" == "$before" ]] || fail 'reinstall changed the executable'

# The default directory is created when missing.
HOME="$qa_dir/home" TMPDIR="$qa_dir" sh "$qa_dir/install.sh"
[[ "$("$qa_dir/home/.local/bin/suzumushi" --version)" == "suzumushi $VERSION" ]] ||
  fail 'default installation failed'

# A wrong checksum or a foreign suzu must leave the previous install alone.
printf 'previous executable\n' >"$bin/suzumushi"
sed "s|^expected=.*|expected='$(printf '0%.0s' {1..64})'|" "$qa_dir/install.sh" >"$qa_dir/tampered.sh"
if HOME="$qa_dir/home" TMPDIR="$qa_dir" sh "$qa_dir/tampered.sh" --bin-dir "$bin"; then
  fail 'installer accepted a wrong checksum'
fi
[[ "$(cat "$bin/suzumushi")" == 'previous executable' ]] || fail 'checksum failure replaced the executable'
rm "$bin/suzu"
printf 'another program\n' >"$bin/suzu"
if run_installer; then
  fail 'installer replaced a foreign suzu'
fi
[[ "$(cat "$bin/suzu")" == 'another program' ]] || fail 'installer changed a foreign suzu'

for parent in "$qa_dir" "$bin"; do
  if compgen -G "$parent/*suzumushi-install.*" >/dev/null || compgen -G "$parent/.suzumushi-install.*" >/dev/null; then
    fail "installer left temporary state in $parent"
  fi
done
echo "installer_check=pass version=$VERSION"
