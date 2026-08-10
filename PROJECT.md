# Suzumushi

Suzumushi is a small Linux terminal audio player for local files. It has one
job: browse a folder-based library, build a queue, and play it cleanly from the
terminal.

The project is intentionally personal and simple. It has no album artwork,
terminal image protocol, visualizer, tag editor, database, account, cloud
service, or network feature.

## Current status

Version `0.2.0` is released. It contains the safe root initializer,
descriptor-rooted scanner, bounded text metadata reader, terminal shell, file
logging, tests, fuzz targets, and CI foundation.

The development branch has simplified that shell to three panels:

```text
+----------------------+----------------------------+----------------------+
|       Library        |           Player           |        Queue         |
| folders, playlists   | title, creator, progress   | upcoming tracks      |
| and search results   | time, volume, speed        | and queue controls   |
+----------------------+----------------------------+----------------------+
| q quit   Tab focus   Space play/pause   / search   ? help                |
+----------------------------------------------------------------------------+
```

Playback is not implemented yet. The next work is Phase 4.

## Product contract

### Included in v1

- Linux only, fully local, and keyboard accessible.
- Arbitrary folders under `audio/library/`.
- Immediate child folders under `audio/playlists/` as playlists.
- Copied files and file symlinks in playlists.
- Library browsing, local search, and a bounded queue.
- Play, pause, stop, next, previous, seek, volume, mute, shuffle, and repeat.
- Pitch-preserving playback speed from `0.5x` through `2.0x`, including `1.0x`.
- MPRIS, desktop media keys, and text-only track notifications.
- Fast status commands for Waybar, tmux, and Zellij. All three are supported.
- A direct local install command, verified GitHub release binaries, and the
  `suzumushi-bin` AUR package.
- The canonical `suzumushi` executable. Packages may add a relative
  `suzu -> suzumushi` symlink, never a second binary target.

### Deliberately excluded

- Album covers, embedded picture extraction, artwork caches, Kitty, Sixel,
  iTerm2 image support, and `mpris:artUrl`.
- Audio visualizers and decorative playback effects.
- Tag editing, batch marking, backup journals, and media-file mutation.
- Lyrics, streaming services, RSS, downloads, accounts, cloud sync, remote
  control, and any IPv4 or IPv6 runtime access.
- SQLite, playlist files, and live filesystem watching in v1.
- A custom Zellij plugin. The normal status command is enough.

The mascot remains part of the project identity. It belongs in release assets
and the future landing page, not in the terminal playback path.

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

The scanner performs one deterministic descriptor-rooted traversal per request.
It never follows directory symlinks. File symlinks are opened with Linux secure
resolution and no weaker path fallback. Audio extensions are discovery hints,
not codec support claims.

Metadata scanning reads only artist, album artist, album, and title. Audio
properties and embedded pictures are disabled. Parsing runs in a bounded helper
process with read, operation, output, file descriptor, memory, CPU, and wall-time
limits.

## Architecture

- The terminal loop exclusively owns `AppState`, focus, library selection,
  search state, queue, history, shuffle, repeat, queue generations, and playback
  generations.
- The audio worker owns the decoder, output device, and current playback state.
  It never chooses the next queue item.
- Workers use bounded messages, explicit cancellation, and awaited shutdown.
- Reliable controls and state updates use a bounded lane. Position is a
  capacity-one latest-value update.
- The real-time audio callback never allocates, blocks, logs, touches D-Bus,
  takes a lock, or sends on a blocking channel.
- The session retains the selected root descriptor and filesystem identity.
  Config, locking, scanning, logging, and later state files are opened relative
  to that descriptor.
- `waybar`, `tmux`, and `zellij` will be read-only renderers of one bounded,
  versioned `state/now-playing.json`. They never scan or start audio.
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
- Terminal, Waybar/Pango, tmux, Zellij, notification, JSON, and file URI output
  each receive their own bounded escaping rules when implemented.
- Known dependency advisories are denied. `RUSTSEC-2024-0436` is temporarily
  accepted because it reports unmaintained `paste 1.0.15`, reached only through
  Lofty, not a vulnerability. Review or remove the exception by 2026-11-09.

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
the same task. The four current fuzz targets cover path classification, config,
the metadata adapter, and terminal-safe text.

Tests stay behavior-focused and deterministic. No sleeps, retry-to-green, or
coverage targets. Audio behavior must run in CI with a fake device. Real device
and packaged artifact checks belong in focused release QA.

## Roadmap

### Phase 4: library, search, and queue

**Status:** next

- Move the existing scan index into app-owned state.
- Render the library tree and folder playlists in the Library panel.
- Add local `/` search over the existing metadata, filename, and relative-path
  fields.
- Add bounded queue insert, remove, reorder, clear, and selection behavior.
- Keep deterministic natural ordering and stable asset, entry, and playlist IDs.
- Defer batch marking and playback transition machinery.

Done when a user can initialize a root, add files, open the TUI, browse and
search them, build a queue, and quit cleanly without an audio device.

### Phase 5: one audio pipeline

**Status:** not started

- Run a short compatibility spike and choose one production decode and output
  path. Do not keep two pipelines.
- Advertise only container and codec combinations verified through that exact
  path.
- Add one owned audio worker with bounded commands and fake-device tests.
- Implement play, pause, stop, next, and previous at `1.0x`.
- Keep queue navigation and generation allocation in the app loop.

Done when a queued local file plays through the selected pipeline and every
basic transition is deterministic under fake-device tests and real-device QA.

### Phase 6: complete playback controls

**Status:** not started

- Add volume, mute, seek, progress, elapsed time, shuffle, and repeat.
- Add pitch-preserving speeds across `0.5x..=2.0x`. This is required, not an
  optional spoken-audio extra.
- Fill the Player panel with title, creator, progress, elapsed and duration,
  volume, mute, state, and speed.
- Write the bounded, versioned `state/now-playing.json` projection.

Done when the terminal is a complete local player and position remains correct
through pause, seek, speed changes, next, previous, and end of track.

**Milestone:** playable core, planned `0.3.0`.

### Phase 7: Waybar, tmux, and Zellij

**Status:** not started

- Add all three fast renderer commands from the same state file.
- Keep outputs single-line, bounded, stale-aware, and specific to each
  formatting language.
- Provide working snippets for Waybar, tmux, and `zjstatus`.

Done when every renderer works without starting the TUI, scanning, or opening
an audio device.

### Phase 8: MPRIS and media keys

**Status:** not started

- Add one MPRIS identity on the winning terminal session.
- Route media keys and `playerctl` through normal app actions.
- Validate stale track IDs and playback generations for seeking.
- Publish text metadata only. Notifications are text only.

Done when play, pause, stop, next, previous, seek, volume, and metadata work
through MPRIS without a second playback state.

**Milestone:** desktop and status integrations, planned `0.4.0`.

### Phase 9: terminal polish and local installation

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

### Phase 10: Linux release and AUR

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
