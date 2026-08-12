# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Published the verified `suzumushi-bin` package to AUR for Arch Linux.

## [1.0.0] - 2026-08-12

### Added

- Added session-only Library folder collapse without hiding search results or
  changing the Queue or playback.

### Changed

- Aligned the Library, Player, and Queue panel titles to the top-left edge.
- Marked the focused panel with `>` and a bold single-line border instead of
  changing the border shape.
- Made `Enter` play or pause outside Library and separated footer bindings with
  calm middle dots, with distinct `p` previous and `n` next labels.

### Fixed

- Prevented the same underlying audio asset from being added to the Queue more
  than once while keeping playlist insertion atomic.
- Cleared transient informational footer notices after three seconds while
  keeping warnings and errors visible.
- Made release publication read annotated notes from GitHub without checking
  executable source into the write-capable job.
- Updated artifact transfer actions to their pinned Node 24 releases.

## [1.0.0-rc.1] - 2026-08-11

### Added

- Added tag-driven, checksummed Linux release archives built with Rust 1.97.1.
- Added the canonical `suzumushi` executable and relative `suzu -> suzumushi` packaging symlink to release archives.

### Fixed

- Kept unsupported AAC and M4A files out of the library while retaining Ogg audio discovered through the `.oga` extension.
- Made atomic state writes skip a bounded number of abandoned temporary names without deleting unknown files.
- Released root and active-session leases explicitly before closing their descriptors so concurrent same-process handoffs remain deterministic.

## [0.4.0] - 2026-08-10

### Added

- Added one MPRIS identity for the active terminal session with global media-key and `playerctl` control for playback, navigation, seeking, volume, shuffle, and repeat.
- Added bounded text metadata and generation-aware track identities without artwork or file URLs.
- Added a complete built-in key guide, explicit empty and failure guidance, command version output, and clearer initialization instructions.
- Added a reproducible local-install check that installs the current checkout, initializes a root, and scans a real fixture through the installed binary.

### Changed

- Routed every desktop control through the existing app-owned actions and published app state through a capacity-one latest projection instead of creating a second playback state.
- Made Space respond immediately by removing the unused command-palette leader delay. Released configuration containing the old timeout remains narrowly compatible.
- Stored metadata once per canonical asset and gave contextual library and playlist entries a direct immutable relationship to it.
- Named informational, warning, and error notices in text and kept loading, playback, help, empty, and small-terminal states useful at the supported minimum size.

### Fixed

- Made the MPRIS worker release its D-Bus connection and join promptly when the terminal quits.
- Preserved paused and stopped playback across MPRIS navigation, kept paused tracks pausable, advanced oversized relative seeks like Next, and ignored oversized absolute positions.
- Made the CPAL stream lifecycle idempotent so consecutive navigation while paused cannot fail by pausing an inactive replacement stream.

## [0.3.0] - 2026-08-10

### Added

- Added a nested local library and folder-playlist browser backed by the existing descriptor-rooted scan index.
- Added bounded local search over metadata, filenames, and relative paths.
- Added app-owned queue selection, insertion, removal, reordering, and clearing with checked item and byte limits.
- Added basic `1.0x` play, pause, stop, next, previous, and automatic queue advancement.
- Added one isolated Symphonia decoder, one CPAL output path, a lock-free PCM ring, and deterministic fake-device tests.
- Added verified MP3, FLAC, WAV/PCM, and Ogg/Vorbis compatibility fixtures and a bounded decoder fuzz target.
- Added volume, mute, five-second seeking, progress, shuffle, and repeat off/queue/one.
- Added a bounded, private, versioned `state/now-playing.json` playback projection.
- Added bounded automatic restoration of the last Queue, current track, and playback position in a paused state.
- Added an original terminal-native Suzu animation that dances during playback and rests in every non-playing state.

### Changed

- Made local search linear in retained field bytes per query update and clarified its modal quit keys.
- Simplified the terminal to three full-height Library, Player, and Queue panels, with the Player receiving half the terminal width.
- Terminal startup now transfers its single scan result into the event loop before drawing the library.
- Reduced configuration to behavior that exists today; later phases will add settings with their features.
- Renamed the root mutation lease to the root writer lease to match its remaining state and logging role.
- Completed the Player panel with title, creator, progress, elapsed and total time, volume, mute, playback state, shuffle, and repeat.
- Made Enter start a newly queued selection when playback is idle while retaining normal append behavior during playback.
- Made next, previous, shuffle, and repeat controls permanently visible in the terminal footer.
- Made repeat control permanently visible and named whole-Queue repetition `queue`.
- Omitted the creator line when a track has no artist or album-artist metadata.

### Fixed

- Bounded playback-state identity fields for their worst-case JSON escaping and kept a five-second heartbeat while the TUI is alive.
- Published a final stopped state after event-loop failures whenever the state directory remains writable.
- Discarded decoder preroll on seeks and ordered reliable restart events against coalesced position updates.
- Coalesced held-arrow seek targets so decoder restarts cannot block the terminal, overwrite the newest target, or swallow pause and resume.
- Preserved shuffle history across queue edits and started new shuffled playlists at their first selected track without skipping the rest.
- Kept long timeline text visible at 80 columns and removed a scheduler-dependent PTY assertion.

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
