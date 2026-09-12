# Linux release and AUR packaging

Suzumushi publishes one prebuilt target:
`x86_64-unknown-linux-gnu`. The archive is built on Ubuntu 24.04 with Rust
1.98.1 from the exact tagged commit. It contains the canonical `suzumushi`
binary, the relative `suzu -> suzumushi` symlink, the README, changelog, and
Apache 2.0 license.

The binary dynamically uses glibc, libgcc, ALSA, PipeWire, and D-Bus. It has no
network runtime. The release target is verified on the GitHub runner and current
Arch Linux; it is not a musl, ARM, macOS, or Windows compatibility claim.

PipeWire and D-Bus are linked unconditionally, not loaded on demand: the dynamic
linker resolves `libpipewire-0.3.so.0` and `libdbus-1.so.3` before any backend
selection happens, so the ALSA fallback does not make them optional at runtime.
Releases before 1.1.0 linked only ALSA.

## GitHub release procedure

The release operator must start from a clean commit on `shrek` whose version,
changelog, and documentation agree. Run `mise run ci`, the local installation
QA, and the release archive QA before pushing the commit. Wait for CI on that
exact pushed commit to pass.

Create and push one annotated `v<version>` tag. Never move or reuse a release
tag. If a candidate is wrong, fix the source and publish the next candidate.

The tag starts `.github/workflows/release.yml`. Its build job has read-only
repository permission and performs these operations:

1. Require the tag to equal `v` plus the Cargo package version.
2. Run the complete `mise run ci` gate.
3. Require Rust 1.98.1 and build the locked GNU/Linux release binary.
4. Create and independently inspect the fixed-content archive.
5. Produce and verify `SHA256SUMS`.

A separate job receives only those verified files and the permission needed to
create the GitHub release. Prerelease versions are marked as prereleases and
are never selected as the latest stable release. The workflow refuses to
overwrite existing archive or checksum files.

If only publication fails after the build job succeeds, download that run's
named artifact, verify it locally, and publish those exact files from a clean
checkout containing the annotated tag. Never rebuild the recovery artifact or
move the tag.

After publication, download both assets into a new temporary directory, run
`scripts/verify-release-archive.sh`, and exercise the extracted binary and
relative symlink. Record the tag, commit, workflow result, archive SHA-256, and
QA environment.

## AUR package publication

After each GitHub release is published, its exact checksum is staged in
`packaging/aur/`, the source copy for the `suzumushi-bin` AUR repository. That
directory must contain only `PKGBUILD` and its generated `.SRCINFO`. The recipe
consumes the published GitHub archive by exact SHA-256 and lets `makepkg` strip
unneeded symbols from the installed executable.

No AUR credential belongs in this repository or in GitHub Actions. If AUR
writes are unavailable, do not probe authentication, create a repository, or
push updates. Keep the prepared files committed here and stop at local package
verification.

When writes are available:

1. Confirm the AUR service notice is cleared through official Arch channels.
2. Clone `ssh://aur@aur.archlinux.org/suzumushi-bin.git` into a new directory.
3. Copy only `PKGBUILD` and `.SRCINFO` from `packaging/aur/`.
4. Review the complete diff and run `makepkg --verifysource`.
5. Build in a clean Arch environment and run `namcap` on the `PKGBUILD` and
   built package.
6. Test clean installation, both command names, upgrade, and uninstall.
7. Commit the two packaging files and push once through the configured AUR key.

For a stable release, update the Cargo version and changelog first. Publish and
verify the final GitHub artifact, then replace the prior version, URL, and
checksum in the AUR recipe. Regenerate `.SRCINFO`; never edit it by hand.

`depends` must match what the published archive actually links, which is why it
is only ever changed together with the archive it describes. The recipe includes
`libpipewire` and `dbus` alongside `alsa-lib`, `glibc`, and `libgcc`; without
the PipeWire runtime library the installed binary fails at exec with
`error while loading shared libraries: libpipewire-0.3.so.0`. Confirm the list
against `ldd` on the extracted release binary, and let `namcap` corroborate it.
