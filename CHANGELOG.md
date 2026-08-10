# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Added a nested local library and folder-playlist browser backed by the existing descriptor-rooted scan index.
- Added bounded local search over metadata, filenames, and relative paths.
- Added app-owned queue selection, insertion, removal, reordering, and clearing with checked item and byte limits.
- Added basic `1.0x` play, pause, stop, next, previous, and automatic queue advancement.
- Added one isolated Symphonia decoder, one CPAL output path, a lock-free PCM ring, and deterministic fake-device tests.
- Added verified MP3, FLAC, WAV/PCM, and Ogg/Vorbis compatibility fixtures and a bounded decoder fuzz target.
- Added volume, mute, five-second seeking, progress, shuffle, repeat off/all/one, and pitch-preserving `0.5x` through `2.0x` playback.
- Added a bounded, private, versioned `state/now-playing.json` projection for later status integrations.

### Changed

- Made local search linear in retained field bytes per query update and clarified its modal quit keys.
- Simplified the terminal to three full-height Library, Player, and Queue panels, with the Player receiving half the terminal width.
- Terminal startup now transfers its single scan result into the event loop before drawing the library.
- Reduced configuration to behavior that exists today; later phases will add settings with their features.
- Consolidated the product contract, safety notes, verification rules, and roadmap into `PROJECT.md` and `AGENTS.md`.
- Renamed the root mutation lease to the root writer lease to match its remaining state and logging role.
- Completed the Player panel with title, creator, progress, elapsed and total time, volume, mute, playback state, speed, shuffle, and repeat.
- Made Enter start a newly queued selection when playback is idle while retaining normal append behavior during playback.

### Fixed

- Bounded playback-state identity fields for their worst-case JSON escaping and kept a five-second heartbeat while the TUI is alive.
- Published a final stopped state after event-loop failures whenever the state directory remains writable.
- Discarded decoder preroll on seeks and ordered reliable restart events against coalesced position updates.
- Preserved shuffle history across queue edits and started new shuffled playlists at their first selected track without skipping the rest.
- Honored playback speed for sub-window clips, kept long timeline text visible at 80 columns, and removed a scheduler-dependent PTY assertion.

### Removed

- Removed cover discovery, artwork state directories, artwork configuration, and every planned artwork cache, MPRIS art URL, and notification image requirement.
- Removed Kitty, Sixel, and iTerm2 probing, terminal image protocol state, and the terminal capability fuzz target.
- Removed the visualizer and tag-editing roadmap, configuration, backup tree, and mutation-journal plans.
- Removed the obsolete standalone documentation set and `SECURITY.md`; current contracts live in the project source of truth, with one focused parser-isolation decision.

## [0.2.0] - 2026-08-10

### Added

- Initial Rust package with a canonical `suzumushi` command and library target.
- Command-line help for the canonical executable.
- Reproducible formatting, linting, test, and MSRV checks through `mise` and CI.
- Safe root initialization with strict configuration, private state, logs, and backup storage.
- Descriptor-rooted local media diagnostics for library files, folder playlists, file symlinks, artwork candidates, metadata, and bounded warnings.
- Per-root mutation and per-user active-TUI lock primitives with verified runtime storage.
- Dependency security checks and bounded fuzz targets for current untrusted-input boundaries.
- Root security policy with private reporting guidance and the current trust-boundary register.
- A keyboard-only terminal interface with four focused panels, a compact help bar, deterministic small-terminal fallback, and clean `q` exit.
- Bounded private file logging with record truncation, counted queue drops, and retained-size rotation.
- Bounded terminal image-capability discovery with safe fallback when terminals or multiplexers do not answer.

### Changed

- Kept Now Playing and Art together in the middle column, with Queue using the full-height right panel.
- Centered panel titles and kept their labels stationary while focus moves.
- Defined Now Playing as a compact track identity and Art as the persistent cover, album, playback, progress, timer, and visualizer surface.

### Fixed

- Skipped hidden directory subtrees before their descendants can consume scan budgets.
- Rendered malformed command-line arguments with one clean error prefix.
- Prevented FIFO and raced special-file entries from blocking media scans, configuration loading, or repeated root initialization.
- Reported exact nested paths when repeated initialization encounters an invalid directory or file.
- Rejected process budgets that cannot hold both scan indexes and the required scanner/parser scratch space.
- Removed repeated full asset searches from scanning and large-library diagnostics.
- Applied deterministic natural ordering to each nested path component before the exact raw-path tie-breaker.
- Rendered invalid Linux filename bytes with reversible terminal-safe escapes distinct from literal escape text.
- Kept terminal capability replies out of keyboard input by issuing one query and accepting only a complete response frame.
- Drained and joined the owned logging worker before reporting background failures, emitted dropped-record warnings outside the lossy queue, and pruned archives above lowered retention settings.
- Resolved invalid, modified, and double-Space leader sequences to one action without accepting modified chord keys.
- Bounded both terminal cell buffers before initial allocation and resize, with clean restoration when a reported terminal size exceeds the UI reservation.
- Required initialization to verify and acquire the root writer lease before repairing an existing root.
- Reserved the terminal session's five-descriptor startup peak before opening locks or log storage.
- Disabled embedded cover-art parsing during text-only metadata scans.
- Kept the selected root descriptor and filesystem identity pinned across configuration, locking, scanning, and logging for the whole command or terminal session.
