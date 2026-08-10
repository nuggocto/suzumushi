# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
