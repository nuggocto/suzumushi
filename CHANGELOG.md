# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Initial Rust package with a canonical `suzumushi` command and library target.
- Command-line help for the canonical executable.
- Reproducible formatting, linting, test, and MSRV checks through `mise` and CI.
- Safe root initialization with strict configuration, private state, logs, and backup storage.
- Descriptor-rooted local media diagnostics for library files, folder playlists, file symlinks, lyrics, artwork candidates, metadata, and bounded warnings.
- Per-root mutation and per-user active-TUI lock primitives with verified runtime storage.
- Dependency security checks and bounded fuzz targets for current untrusted-input boundaries.
- Root security policy with private reporting guidance and the current trust-boundary register.

### Fixed

- Skipped hidden directory subtrees before their descendants can consume scan budgets.
- Rendered malformed command-line arguments with one clean error prefix.
- Prevented FIFO and raced special-file entries from blocking media scans, configuration loading, or repeated root initialization.
- Reported exact nested paths when repeated initialization encounters an invalid directory or file.
- Rejected process budgets that cannot hold both scan indexes and the required scanner/parser scratch space.
- Removed repeated full asset searches from scanning and large-library diagnostics.
- Applied deterministic natural ordering to each nested path component before the exact raw-path tie-breaker.
- Rendered invalid Linux filename bytes with reversible terminal-safe escapes distinct from literal escape text.
