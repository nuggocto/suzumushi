# Suzumushi

Suzumushi is a small Linux terminal audio player for local files. It has one
job: browse a folder-based library, build a queue, and play it cleanly from the
terminal.

The project is intentionally personal and simple. It has no album artwork,
terminal image protocol, visualizer, tag editor, database, account, cloud
service, or network feature. Suzu's small terminal-native animation is compiled
into the player and reacts only to playback state.

## Current status

Version `0.2.0` is released. The development branch adds the complete local
library, search, queue browser, and playback controls to its safe root,
scanner, metadata, terminal, logging, test, fuzz, and CI foundation.

```text
+------------------+----------------------------------------+------------------+
|     Library      |                 Player                 |      Queue       |
| folders, tracks  |     title, creator, progress, time     |  ordered tracks  |
| playlists/search |        volume, state, dancing Suzu     | and queue edits  |
+------------------+----------------------------------------+------------------+
| q quit   Tab focus   / search   Enter add   Up/Down move                     |
| Space play/pause  s stop  n next  p previous  x shuffle  r repeat             |
+------------------------------------------------------------------------------+
```

The complete local player is implemented. The next work is Phase 7.

## Product contract

### Included in v1

- Linux only, fully local, and keyboard accessible.
- Arbitrary folders under `audio/library/`.
- Immediate child folders under `audio/playlists/` as playlists.
- Copied files and file symlinks in playlists.
- Library browsing, local search, and a bounded queue.
- Play, pause, stop, next, previous, seek, volume, mute, shuffle, and repeat.
- An original fixed-size Suzu character animation in the Player. Suzu dances
  only while audio is playing and returns to one calm resting frame otherwise.
- Automatic restoration of the last non-empty queue, current track, and
  playback position. Restored sessions remain paused until the user presses
  Space.
- MPRIS and global media-key support.
- A direct local install command, verified GitHub release binaries, and the
  `suzumushi-bin` AUR package.
- The canonical `suzumushi` executable. Packages may add a relative
  `suzu -> suzumushi` symlink, never a second binary target.

### Deliberately excluded

- Album covers, embedded picture extraction, artwork caches, Kitty, Sixel,
  iTerm2 image support, and `mpris:artUrl`.
- PCM-reactive visualizers and decorative effects derived from audio data.
- Tag editing, batch marking, backup journals, and media-file mutation.
- Lyrics, streaming services, RSS, downloads, accounts, cloud sync, remote
  control, and any IPv4 or IPv6 runtime access.
- SQLite, playlist files, and live filesystem watching in v1.
- Waybar, tmux, and Zellij status renderers.
- Desktop notifications.

The mascot remains part of the project identity. Her small original terminal
animation ships with the player; richer mascot art belongs in release assets
and the future landing page.

## Filesystem model

Root selection is exactly:

1. `--root <path>`
2. `SUZUMUSHI_ROOT`
3. `./suzumushi` only when `./suzumushi/audio` already exists

There is no fallback to `~/Music`.

New roots use this small layout:

```text
suzumushi/
├── audio/
│   ├── library/
│   └── playlists/
├── state/
├── logs/
└── config.toml
```

The root lock file is app-owned and hidden. Roots created by `0.2.0` may still
contain now-unused `state/artwork/` and `backups/` directories. New code does
not read or write them. Their released configuration is read through a narrow
migration that discards only the retired prototype fields. New configuration is
versioned and remains strict.

`state/session.json` is the single private, bounded, versioned resume
checkpoint. It stores stable entry identifiers rather than media paths. Queue
changes are checkpointed immediately, playback position is checkpointed at a
bounded interval and on shutdown, and clearing the Queue removes the
checkpoint. Missing entries are skipped after the next scan; malformed,
oversized, or unsupported safe checkpoint files are ignored with a visible
warning. Unsafe file identities still fail closed.

The scanner performs one deterministic descriptor-rooted traversal per request.
It never follows directory symlinks. File symlinks are opened with Linux secure
resolution and no weaker path fallback. Audio extensions are discovery hints,
not codec support claims.

Metadata scanning reads only artist, album artist, album, and title. Audio
properties and embedded pictures are disabled. Parsing runs in a bounded helper
process with read, operation, output, file descriptor, memory, CPU, and wall-time
limits.

Search is modal and ASCII-case-insensitive. Each query update uses a linear-time
matcher over the retained bounded fields. While search is open, `q` remains
query text; `Esc` closes search and `Ctrl+c` quits globally.

The Player shows artist or album-artist metadata when present. Missing creator
metadata is omitted rather than replaced with a placeholder.

Left and Right seek in five-second steps. Repeated input retains the newest
app-owned target until the worker acknowledges it; obsolete absolute targets
are coalesced instead of queueing decoder restarts.

## Architecture

- The terminal loop exclusively owns `AppState`, focus, library selection,
  search state, queue, history, shuffle, repeat, queue generations, and playback
  generations.
- The audio worker owns the decoder, output device, and current playback state.
  It never chooses the next queue item.
- Workers use bounded messages, explicit cancellation, and awaited shutdown.
- Reliable discrete controls and state updates use a bounded lane. Absolute
  seek requests and playback positions each use independent capacity-one
  latest-value lanes. Timeline revisions prevent feedback from an older seek
  from replacing the newest app-owned target.
- The real-time audio callback never allocates, blocks, logs, touches D-Bus,
  takes a lock, or sends on a blocking channel.
- The session retains the selected root descriptor and filesystem identity.
  Config, locking, scanning, logging, and state files are opened relative
  to that descriptor.
- The future MPRIS adapter will route D-Bus requests into the existing app-owned
  playback actions. It will not create a second playback state.
- Suzu's frame is derived from the app-owned playback status and the terminal's
  existing monotonic tick. It uses no worker, decoder data, media artwork, or
  additional dependency.
- `src/main.rs` remains thin. Behavior exposed to integration tests lives in
  the library target. A module is added only with its first real behavior.

The terminal inherits the user's foreground and background. It uses default or
named ANSI colors only, honors `NO_COLOR` and `theme = "mono"`, and never relies
on color alone for focus or state.

## Safety boundaries

Local does not mean trusted. Media, metadata, filenames, configuration, state,
filesystem entries, and later D-Bus arguments are bounded before allocation or
expensive work.

- Application Rust forbids `unsafe`.
- Direct dependencies are exact-version pinned with minimal features.
- Root, audio, config, lock, log, and state operations use descriptor-relative
  no-follow access where identity matters.
- The current-UID active TUI lock lives below verified `$XDG_RUNTIME_DIR`.
  There is no `/tmp` fallback and stale files are never blindly deleted.
- Terminal, JSON, file URI, and later D-Bus values each receive bounded
  validation or escaping appropriate to their destination.
- Known dependency advisories are denied. `RUSTSEC-2024-0436` is temporarily
  accepted because it reports unmaintained `paste 1.0.15`, reached only through
  Lofty, not a vulnerability. Review or remove the exception by 2026-11-09.

Search queries are capped at 4 KiB and return at most 200 naturally ordered
results. Queue storage is capped by both item count and reserved bytes before a
mutation. Existing version 1 configuration files receive these current defaults;
unknown fields remain rejected.

Security reports should use GitHub private vulnerability reporting. Do not put
vulnerability details or secrets in a public issue.

## Toolchain and commands

- Development and release Rust: `1.97.1`
- MSRV: `1.95.0`
- Edition: `2024`
- Resolver: `3`
- License: `Apache-2.0`
- One root package and one canonical binary
- No Cargo features in the application
- Source builds require the distribution's ALSA development files. CI installs
  Ubuntu's `libasound2-dev` explicitly.

`mise.toml` is the command authority:

```sh
mise run fmt
mise run clippy
mise run test
mise run msrv
mise run security
mise run fuzz-smoke
mise run ci
```

`mise run ci` runs those checks in that order. CI uses one workflow that calls
the same task. The five current fuzz targets cover path classification, config,
the metadata adapter, audio decoding, and terminal-safe text.

Tests stay behavior-focused and deterministic. No sleeps, retry-to-green, or
coverage targets. Audio behavior must run in CI with a fake device. Real device
and packaged artifact checks belong in focused release QA.

## Roadmap

### Phase 4: library, search, and queue

**Status:** complete

- [x] Move the existing scan index into app-owned state.
- [x] Render the library tree and folder playlists in the Library panel.
- [x] Add local `/` search over the existing metadata, filename, and relative-path
  fields.
- [x] Add bounded queue insert, remove, reorder, clear, and selection behavior.
- [x] Keep deterministic natural ordering and stable asset, entry, and playlist
  IDs.
- [x] Defer batch marking and playback transition machinery.

Done when a user can initialize a root, add files, open the TUI, browse and
search them, build a queue, and quit cleanly without an audio device.

Verified with focused state and adversarial search tests, reviewed 80x24 and
120x32 snapshots, the full `mise run ci` sequence, and a release-profile PTY
journey over a synthetic nested `JDR` library.

### Phase 5: one audio pipeline

**Status:** complete

- [x] Run a short compatibility spike and choose one production decode and output
  path. Do not keep two pipelines.
- [x] Advertise only container and codec combinations verified through that exact
  path.
- [x] Add one owned audio worker with bounded commands and fake-device tests.
- [x] Implement play, pause, stop, next, and previous at `1.0x`.
- [x] Keep queue navigation and generation allocation in the app loop.

Done when a queued local file plays through the selected pipeline and every
basic transition is deterministic under fake-device tests and real-device QA.

Verified with MP3, FLAC, WAV/PCM, and Ogg/Vorbis fixtures through Symphonia
0.6.0; helper crash, timeout, malformed-input, and descriptor-revalidation
tests; deterministic fake-device state and drain tests; the bounded decoder
fuzz target; the full `mise run ci` sequence; and an ignored PTY journey run
against the local PipeWire-backed Linux output device.

### Phase 6: complete playback controls

**Status:** complete

- [x] Add volume, mute, seek, progress, elapsed time, shuffle, and repeat.
- [x] Fill the Player panel with title, creator, progress, elapsed and duration,
  volume, mute, state, shuffle, and repeat.
- [x] Write the bounded, versioned `state/now-playing.json` projection.
- [x] Restore the last queue, current track, and position without starting
  playback automatically.
- [x] Add the original terminal-native Suzu animation with a deterministic
  resting frame and bounded playing loop.
- [x] Coalesce repeated absolute seeks without blocking the terminal or losing
  pause and resume during a decoder restart.

Done when the terminal is a complete local player and position remains correct
through pause, seek, next, previous, end of track, and a clean close/reopen
cycle.

Verified with app-owned control, shuffle-history, repeat, generation, and
cross-lane ordering tests; preroll-free accurate seeking through every verified
container; repeated-seek coalescing and target-acknowledgement tests;
deterministic fake-device gain, restart, pause-during-restart, position, and
drain tests;
bounded escaped-state, atomic replacement, heartbeat, and failure-cleanup tests;
bounded resume-state parsing, stale-entry recovery, clear-state, and
resume-position tests; deterministic playback-state animation tests; reviewed
Suzu cycle, 80x24, and 120x32 snapshots; the full `mise run ci` sequence; a PTY
search, queue, state-file, and close/reopen journey; and the real Linux
audio-device PTY check.

**Milestone:** playable core, planned `0.3.0`.

### Phase 7: MPRIS and global media keys

**Status:** next

- Add one MPRIS identity on the winning terminal session.
- Route global media keys and `playerctl` through normal app actions.
- Validate stale track IDs and playback generations for seeking.
- Publish bounded text metadata only.

Done when play, pause, stop, next, previous, seek, volume, and metadata work
through MPRIS and global media keys without a second playback state.

**Milestone:** desktop controls, planned `0.4.0`.

### Phase 8: terminal polish and local installation

**Status:** not started

- Finish empty, loading, warning, error, help, and small-terminal states.
- Keep the three-panel layout calm and useful at 80x24 and wider sizes.
- Add the simple local installation path:

  ```sh
  cargo install --path . --locked
  ```

- Run real keyboard, audio-device, media-key, and supported terminal QA.

Done when a friend can clone the repository, run one install command, initialize
a root, and use the player without reading project internals.

### Phase 9: Linux release and AUR

**Status:** not started

- Add the single release workflow only when the application is feature complete.
- Build verified `x86_64-unknown-linux-gnu` archives and checksums from tags.
- Publish only `suzumushi-bin` to AUR from those verified artifacts.
- Test clean install, upgrade, uninstall, `suzumushi`, and the relative `suzu`
  symlink.
- Finish release notes, compatibility claims, and concise user documentation.

Done when the tagged artifacts, local install, GitHub release, and AUR package
all install and run the same tested application.

**Milestone:** `1.0.0`.

## Website and mascot

The mascot is **Suzu**, a small, round, calm bell cricket who sings your music
into the night. Suzu is plump and glossy, with soft charcoal-green coloring,
warm amber underwings, long gentle antennae, and a tiny brass hand-bell. The
character is quiet, cozy, and unhurried, usually pictured on a windowsill or a
curled leaf beneath a low warm lamp.

Suzu belongs to the project's identity, release assets, and website. The mascot
also appears in the terminal as original fixed-size character art. The Player
advances Suzu through a small loop only while audio is playing and immediately
returns to the resting frame while paused, stopped, loading, or failed. This is
not an audio visualizer: it never receives PCM, volume, frequency, or media
artwork data.

After v1, create `suzumushi-front` as a separate static Astro repository for
`suzumushi.org`, deployed on Cloudflare Pages. Keep the mascot and the calm
visual identity there. The first site needs only a landing page, install page,
and changelog. It must describe shipped commands and releases only.

No website or Node dependency belongs in this repository.

## Versioning

`CHANGELOG.md` holds user-visible changes. Git tags use a `v` prefix while Cargo
and changelog versions do not. `v0.2.0` is the released terminal foundation.
The next planned milestones are `0.3.0`, `0.4.0`, and `1.0.0` as described
above. A version is tagged only after local CI, real executable QA, and the
exact pushed commit are green.
