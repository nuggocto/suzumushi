# Repository Instructions

## Source Of Truth

- Read `PROJECT.md` before designing or implementing anything; it is currently the detailed specification and project setup is in progress.
- Follow the phase order and the current phase's build steps, `Done when` gates, and `Do not build yet` prohibitions. Do not scaffold the final tree or pull later-phase dependencies forward.
- Keep the `PROJECT.md` progress board, per-phase status, and checkboxes synchronized with evidence. A gate is complete only when its stated verification passes.
- Update `PROJECT.md` and every affected design/security/testing document when a requirement changes; implementation or tests do not silently override the written contract.
- Prefer checked-in manifests, `mise.toml`, CI, and code over roadmap examples when they later disagree; reconcile the roadmap rather than preserving two contracts.

## Current Setup State

- The package, library target, `mise.toml`, CI workflow, root initializer, descriptor-rooted scanner, lock primitives, focused tests, security checks, and bounded fuzz targets exist. Add later modules only when the current roadmap phase first needs their behavior.
- Keep the root `suzumushi` package as the only package. Do not add helper packages or a `fuzz/` workspace until a current feature needs them.
- Use Rust 1.97.1 for development/release, Rust 1.95.0 as MSRV, edition 2024, resolver 3, and Apache-2.0. Set `rust-version = "1.95.0"`, commit `Cargo.lock`, forbid application `unsafe`, and add SPDX headers to Rust sources.
- Add dependencies only when the current phase needs them, pinned to exact versions and minimal features after the relevant ADR/security review. The initial application has no Cargo features.
- The package has one canonical `suzumushi` binary and one library target. `suzu` is only a relative packaging symlink; never add a second binary target.
- The AUR publishes only `suzumushi-bin` from verified release artifacts. Do not add or document a source-building `suzumushi` AUR package.

## Commands After Setup

- `mise.toml` is the command authority. Normal checks use Rust 1.97.1; only the `msrv` task selects Rust 1.95.0 explicitly.
- Run focused tests with `mise exec -- cargo test --locked -p suzumushi --test <target> <filter>`, then run the relevant `mise` task.
- Full verification is `mise run ci`, in this order: `fmt`, `clippy`, `test`, `msrv`, `security`, and `fuzz-smoke`. Add fault, release, and packaged QA tasks only when their corresponding feature exists.
- CI remains one `.github/workflows/ci.yml` that installs pinned `mise` and calls `mise run ci`; add checks to `mise.toml`, not new workflows. Phase 13 adds the only other workflow, `release.yml`.

## Product Boundaries

- V1 is a Linux-only, fully local terminal audio player. No IPv4/IPv6 runtime access, streaming services, accounts, cloud sync, RSS/download management, network control, or remote APIs; local D-Bus/audio `AF_UNIX` sockets are allowed.
- Root discovery is exactly `--root`, then `SUZUMUSHI_ROOT`, then `./suzumushi` only if `./suzumushi/audio` exists, otherwise a clear init error. Never scan or fall back to `~/Music`.
- The filesystem is the library: arbitrary folders under `audio/library/`, immediate child folders under `audio/playlists/` as playlists, copied or file-symlink entries, natural deterministic order. Do not add SQLite, playlist files, or inotify in v1.
- Audio extensions are discovery hints, not codec support claims. Advertise a container/codec combination only after it passes the versioned compatibility matrix through the selected production pipeline.
- Preserve the calm terminal-native UI: default terminal foreground/background and named ANSI colors only, no hard-coded RGB, honor `NO_COLOR`/`mono`, and keep every action keyboard-accessible.
- `suzumushi-front` is a separate post-v1 static Astro/Cloudflare Pages repository for `suzumushi.org`. Do not scaffold website code or Node dependencies in this repository; publish only install/changelog claims already supported by tagged app releases.

## Working Style

- Use the `rust` skill for every Rust implementation or review.
- Use the `test-quality` skill when writing, changing, or reviewing tests. Add the smallest test that protects meaningful behavior at the lowest useful level; do not duplicate it across test layers, test trivial code, or chase coverage numbers.
- Use the `qa` skill only for real-user verification. Exercise the affected supported flow and credible failures; do not turn every change into full release QA.
- Use the `security` skill for security reviews and changes at actual trust boundaries such as untrusted parsers, filesystem mutation, D-Bus, dependencies, and releases. Keep the review threat-driven and proportional; do not apply generic hardening checklists to unrelated work.
- Roadmap phase labels and numbers are planning metadata. Keep them in `PROJECT.md` and related planning records only; never put `Phase 2`, `Phase 4`, or similar labels in commit messages, code comments, changelog entries, or user-facing output. Commits describe the change, and comments explain the code's invariant or reason.
- Apply TigerStyle only to source-code comments and Rust doc comments. Add a comment when it preserves a non-obvious reason, invariant, bound, safety property, or test method; never narrate syntax the code already makes clear. Keep comments short and plain, with a little playfulness allowed when the stakes are low and clarity remains immediate. Keep security, failure, and recovery comments serious. This comment rule does not extend the TigerStyle voice to documentation, commits, identifiers, machine output, or user replies.
- Load and follow the `vincent` skill for user replies, commit messages, and human-facing prose whenever the format permits it. Keep commit subjects concise and factual. For every non-trivial commit, add a short plain-language body that explains what changed and why; omit the body only when the subject fully explains a truly trivial change. Do not force that voice into code, commands, machine-readable output, legal text, or verbatim quotations.
- Build only the current phase and choose the smallest correct implementation. Avoid speculative abstractions, helper crates, future-feature scaffolding, and performance tooling without a demonstrated problem.

## Architecture Contracts

- The app/UI loop solely owns `AppState`, queue/history/repeat/shuffle, `queue_generation`, and allocation of `playback_generation`. The audio worker owns playback state/device only and never chooses the next queue item.
- Do not make `Arc<Mutex<AppState>>` the architecture. Use bounded message lanes: reliable control/state, capacity-one coalesced position, and independently droppable visualizer telemetry.
- Every worker has one owner, bounded input/output, cancellation, and an awaited shutdown. A Tokio timeout cannot forcibly cancel an uncooperative parser; isolate it in a bounded kill-and-reap helper process when required.
- The real-time audio callback must not allocate, block, log, perform D-Bus work, acquire locks, or send on a blocking channel. The visualizer uses the existing post-volume/mute PCM path and never reopens media or captures system audio.
- `waybar`, `tmux`, and `zellij` are fast read-only renderers of the single bounded/versioned `state/now-playing.json`; they must not scan, initialize audio, start the TUI, or maintain derived state files.
- Create modules only with their first behavior and tests. `src/main.rs` stays thin; `src/lib.rs` owns implementation exposed to integration tests and fuzz targets.

## Filesystem And Mutation Safety

- Treat media, tags, lyrics, artwork, filenames, config/state, journals, backups, and D-Bus input as untrusted and bounded before allocation or expensive work. There is no `unlimited` sentinel.
- Scan exactly once per request from pinned directory descriptors. Use descriptor-relative no-follow traversal; do not replace it with accumulated-path reopen, `std::fs::canonicalize`, or a weaker path fallback.
- Never follow directory symlinks. File symlinks may be read for discovery/playback, but targets outside the canonical root are read/play-only and can never be mutated by confirmation.
- Mutable TUI mode takes the secure per-UID active-TUI lock under verified `$XDG_RUNTIME_DIR` and then the canonical-root writer lock; status commands remain concurrent. Do not use `/tmp` or blindly remove stale locks.
- Lyric/tag/restore writes require identity revalidation, checked budgets, durable journals, verified backups where a preimage exists, same-parent descriptor-relative atomic replacement, and post-write verification. Unsupported semantics fail closed; never downgrade to path-based or non-atomic writes.
- Once a mutation commits, cancellation and the normal shutdown timeout no longer apply: retain locks and await durable completion or a journaled recoverable state. Never detach mutation work or auto-delete recovery evidence/backups.
- Never render raw control characters or reuse terminal escaping for Waybar/Pango, tmux, `zjstatus`, notifications, or file URIs; sanitize and bound each output for its own formatting language.

## Tests And Evidence

- Keep tests focused on current-phase behavior and regressions. Use injected clocks, RNG seeds, identities, generations, and fake audio devices only where nondeterminism is part of that behavior; never synchronize with sleeps or retry failures into green.
- CI audio behavior should run without a sound device. Use the `qa` skill for the few real-device and packaged-artifact checks that automated tests cannot prove.
- Untrusted parser surfaces still need focused malformed/boundary coverage and a completed `docs/parser-isolation.md` decision before third-party code sees untrusted fixtures; keep fuzz targets narrow and non-duplicative.
- Fixtures require recorded source/license/hash. Preserve minimized reproducers and review snapshot diffs for semantic UI changes.
