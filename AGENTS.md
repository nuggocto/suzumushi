# Repository Instructions

## Source of truth

- Read `PROJECT.md` before designing or implementing anything.
- Build phases in order. Follow the current phase's scope and completion gate.
- Keep roadmap status and `CHANGELOG.md` synchronized with verified behavior.
- Reconcile documentation when manifests, code, or tests prove it stale.
- Do not create future modules, dependencies, configuration, or release machinery
  before their first real behavior.

## Product boundaries

- V1 is a Linux-only, fully local terminal audio player. It has no network
  runtime, streaming service, account, cloud sync, database, tag editor, album
  artwork, terminal image protocol, or visualizer.
- Keep the terminal layout to Library, Player, and Queue.
- Root discovery is exactly `--root`, then `SUZUMUSHI_ROOT`, then an existing
  `./suzumushi/audio`. Never fall back to `~/Music`.
- The filesystem is the library. Use folders below `audio/library/` and
  immediate child folders below `audio/playlists/`. Do not add SQLite, playlist
  files, or inotify in v1.
- Keep MPRIS/media keys and all three status outputs: Waybar, tmux, and Zellij.
- Keep the mascot and future `suzumushi-front` website separate from this Rust
  repository.
- Publish only `suzumushi-bin` to AUR from verified release artifacts.

## Rust and repository shape

- Use Rust 1.97.1 for development and release, Rust 1.95.0 as MSRV, edition
  2024, resolver 3, and Apache-2.0.
- Keep `rust-version = "1.95.0"`, commit both lockfiles, forbid application
  `unsafe`, and add SPDX headers to Rust sources.
- Keep one root package, one library target, and one canonical `suzumushi`
  binary. `suzu` may exist only as a relative packaging symlink.
- Pin direct dependencies exactly with minimal features. Add one only when the
  current phase needs it.
- Keep `src/main.rs` thin and expose testable behavior from `src/lib.rs`.

## Commands

- `mise.toml` is authoritative.
- Run focused tests first, then the relevant `mise` task.
- Full verification is `mise run ci`: fmt, clippy, test, MSRV, security, then
  bounded fuzz smoke tests.
- CI remains one `.github/workflows/ci.yml` until the final release phase adds
  `release.yml`.

## Working style

- Use the `rust` skill for every Rust review or implementation.
- Use `test-quality` when tests change, `security` at real trust boundaries,
  and `qa` for real-user verification.
- Load `vincent` for user replies, commit messages, and human-facing prose.
- Phase numbers belong only in `PROJECT.md`, never in commits, source comments,
  changelog entries, or user-facing output.
- Apply TigerStyle only to source and Rust doc comments. Comment non-obvious
  reasons, invariants, bounds, safety properties, and test methods. Do not
  narrate syntax.
- Choose the smallest correct implementation. Avoid speculative abstractions,
  helper crates, and performance work without evidence.

## Architecture contracts

- The app loop solely owns `AppState`, queue, history, shuffle, repeat, queue
  generations, and playback-generation allocation.
- The audio worker owns playback and the device. It never chooses the next queue
  item.
- Do not use `Arc<Mutex<AppState>>`. Use bounded reliable messages and a
  capacity-one latest-position lane.
- Every worker has one owner, bounded input/output, cancellation, and awaited
  shutdown.
- The real-time audio callback never allocates, blocks, logs, performs D-Bus
  work, takes locks, or sends on a blocking channel.
- Waybar, tmux, and Zellij read one bounded versioned
  `state/now-playing.json`. They never scan, start the TUI, or initialize audio.

## Filesystem and parser safety

- Treat media, metadata, filenames, config, state, logs, and D-Bus input as
  untrusted and bounded. There is no unlimited sentinel.
- Keep the selected root descriptor and identity for the whole session. Open
  config, locks, scanning, logging, and state descriptor-relative.
- Scan once per request from pinned directory descriptors. Never replace this
  with accumulated-path reopen, `canonicalize`, or a weaker fallback.
- Never follow directory symlinks. File symlinks are read-only discovery and
  playback inputs.
- Metadata parsing remains isolated in the bounded kill-and-reap helper.
  Embedded cover reading stays disabled.
- Mutable TUI mode takes the verified per-UID active lock and root writer lock.
  Do not use `/tmp` or blindly delete stale locks.
- Sanitize and bound terminal, JSON, Waybar/Pango, tmux, Zellij, notification,
  and file URI output for their own formatting languages.

## Tests

- Add the smallest deterministic test that protects meaningful behavior at the
  lowest useful layer. Do not duplicate it or chase coverage numbers.
- Use injected time, RNG, identity, generation, and fake audio devices where
  nondeterminism is part of the behavior. Never synchronize with sleeps or
  retry failures into green.
- CI audio tests must not require a sound device.
- Keep fuzz targets narrow and bounded. Preserve useful minimized regressions.
- Review snapshot diffs when terminal semantics change.
