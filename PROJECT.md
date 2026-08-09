---
title: Suzumushi - Linux Rust + ratatui Local Audio Player Roadmap
tags: [rust, project, linux, tui, local-audio, music, podcasts, audio-drama, ratatui, mpris, waybar, tmux, zellij, lyrics, visualizer]
created: 2026-07-04
updated: 2026-08-09
status: planning
---

# Suzumushi - Linux Rust + ratatui Local Audio Player

> A calm Linux-only terminal audio player where the user owns a local `suzumushi/audio/` folder, drops supported local audio files and playlist folders into it, plays them from a peaceful `ratatui` interface, sees and edits synced lyrics when present, watches a subtle audio-reactive visualizer, controls playback through media keys, and exposes the current item to Waybar, tmux, Zellij, MPRIS, and desktop notifications.

## Table of Contents

1. [Project Goal](#project-goal)
2. [Name](#name)
3. [Product Shape](#product-shape)
4. [Important Product Decisions](#important-product-decisions)
5. [Controls](#controls)
6. [The Stack](#the-stack)
7. [Architecture Overview](#architecture-overview)
8. [Runtime Model](#runtime-model)
9. [Directory Model](#directory-model)
10. [Config Model](#config-model)
11. [Core Models](#core-models)
12. [Scan Model](#scan-model)
13. [Search Model](#search-model)
14. [Audio Model](#audio-model)
15. [Visualizer Model](#visualizer-model)
16. [Mutation And Backup Model](#mutation-and-backup-model)
17. [Lyrics Model](#lyrics-model)
18. [Tag Model](#tag-model)
19. [Artwork Model](#artwork-model)
20. [MPRIS Model](#mpris-model)
21. [Waybar Model](#waybar-model)
22. [Tmux Model](#tmux-model)
23. [Zellij Model](#zellij-model)
24. [UI Direction](#ui-direction)
25. [Phases](#phases)
26. [Testing Strategy](#testing-strategy)
27. [Safety And Security Notes](#safety-and-security-notes)
28. [Resource Bounds](#resource-bounds)
29. [Release And Packaging](#release-and-packaging)
30. [Final Repo Structure](#final-repo-structure)
31. [Future Ideas After V1](#future-ideas-after-v1)

## Project Goal

Use the `rust` skill for Rust code, `test-quality` when writing or reviewing tests, `qa` for real-user verification, and `security` for actual security-sensitive boundaries or reviews. Keep each review proportional to the current phase: tests protect meaningful behavior, QA exercises supported user flows, and security work focuses on reachable local file, parser, mutation, D-Bus, and release risks rather than generic hardening.

### Requirements and evidence

`PROJECT.md` is the product plan and source of truth until checked-in manifests,
tasks, code, and focused design records implement a requirement more precisely.
When behavior changes, update the roadmap and any affected design, security, or
testing document in the same change instead of preserving two contracts.

Acceptance evidence names the focused test, fixture, or QA step and its pass
criterion. Do not duplicate the same behavior across test layers merely to
increase coverage.

Build a local Rust `ratatui` app that:

- Runs as a Linux command named `suzumushi`
- Uses an app-owned `suzumushi/` root directory
- Uses `suzumushi/audio/` as the only default local-audio area
- Never scans the computer's normal `~/Music` folder unless the user explicitly points or symlinks files there
- Lets users manage music by editing folders directly
- Supports an `audio/library/` folder for the main collection
- Treats every supported file identically at playback time, whether it contains music, a podcast, an audio drama, a tabletop actual-play session, an audiobook, radio, or a personal recording
- Lets users organize `audio/library/` with arbitrary folders; names such as `music/`, `podcasts/`, `audio-series/`, or `D&D/` are conventions, not special media types
- Supports an `audio/playlists/` folder where every child folder is a playlist across all local audio
- Supports copied audio files inside playlists
- Supports symlinked audio files inside playlists
- Refuses to follow symlinked directories in v1 to avoid loops
- Plays songs in natural folder order
- Plays songs in a deterministic shuffle queue
- Supports `/` audio search for creators, titles, albums or series, playlists, filenames, and paths
- Supports `Space+/` as a global search and command palette
- Supports play, pause, next, previous, stop, seek, volume, speed, shuffle, and repeat
- Supports pitch-preserving speed from `0.5x` through `2.0x` for v1
- Shows a small calm audio-reactive mini visualizer in the TUI during playback
- Shows local synced `.lrc` lyrics during playback
- Supports safe local `.lrc` lyric editing
- Exposes MPRIS on Linux so desktop media keys and `playerctl` work
- Exposes album art through MPRIS when available
- Exposes a Waybar custom module command for now-playing status
- Exposes a tmux status command for terminal status bars
- Exposes a Zellij status command for Zellij status bar plugins
- Sends desktop notifications on track change
- Shows album art in terminals that support images
- Supports safe tag viewing for advertised media and single-track/batch tag editing only for container/tag combinations proven writable by the versioned mutation compatibility matrix
- Creates a demo playlist and placeholder instructions during `suzumushi init`
- Uses original calm terminal visuals, not a generic file browser skin
- Stays fully local with no streaming-service connections, accounts, cloud sync, podcast/RSS subscriptions, episode downloading, or other managed downloads
- Ships as a Linux release through cargo, the prebuilt AUR package `suzumushi-bin`, and native Linux prebuilt binaries later; it has no macOS release target

---

## Name

The project name is `Suzumushi` (鈴虫).

Why:

- `Suzumushi` is the bell cricket, a small insect famous for its calm, bell-like song on autumn nights. A peaceful late-night audio player wants exactly that association: a quiet, warm sound that keeps you company after dark.
- It is nocturnal and gentle, which matches the product's mood: quiet, warm, late-night, focused.
- `suzu` (鈴) means "bell", so the idea of a soft, ringing sound is baked into the word.
- It is a distinctive, uncommon word (unlike the earlier `Nagomi`, which was unavailable as a domain), and it still types cleanly as a command.

Domain: `suzumushi.org`, already registered and managed through Cloudflare. The
companion website is intentionally deferred until the application and its real
installation paths are stable.

Command name:

```txt
suzumushi
```

Short alias:

```txt
suzu
```

`suzumushi` is the canonical and only compiled binary. Linux package-manager,
installer, and prebuilt-archive installation flows create `suzu` as a relative
symlink to `suzumushi`; it accepts the same subcommands and flags because it
executes that canonical binary. `cargo install` installs executable targets but
cannot create package symlinks, so the crates.io install path provides
`suzumushi` only and documents an optional user-created `suzu` symlink or shell
alias. Do not add a second Rust binary merely to compensate for that Cargo
limitation.

Project root:

```txt
suzumushi/
```

Audio root:

```txt
suzumushi/audio/
```

### Mascot - "Suzu" the bell cricket

Suzumushi's mascot is a small, round, calm **bell cricket** named Suzu who sings your music into the night.

- A plump, glossy cricket in soft charcoal-green with warm amber underwings and long gentle antennae.
- Holds (or wears at its side) a tiny brass hand-bell; when it "sings", little sound-rings and music notes drift up from the bell.
- Sits on a windowsill or a curled leaf under a low warm lamp, one paper `.lrc` lyric sheet nearby.
- The wings double as a subtle sound-wave / visualizer / progress-bar motif when animated.
- Personality: quiet, cozy, unhurried. Suzu never blares; it hums along at whatever speed you set and keeps you company on late nights.
- TUI motif: a small `♪` trailing from a bell glyph for the now-playing indicator; tiny level bars that breathe with the decoded audio; three faint rings `((•))` as the idle/paused "listening" state; a dim `. z z` when the queue is stopped.

Do not call the binary `suzumushi-player` unless a package registry conflict forces it. The product should feel like one small thing.

---

## Product Shape

Suzumushi is a local Linux TUI audio player, not a streaming client, podcast client, download manager, or library manager that takes ownership away from the filesystem. If a supported audio file already exists on the user's filesystem, Suzumushi can play it regardless of whether it is music, a podcast episode, an audio drama, an actual-play recording, an audiobook, radio, or a personal recording.

Recommended first user flow:

```txt
mkdir -p suzumushi/audio/library/music suzumushi/audio/playlists/focus
cp ~/Downloads/song.flac suzumushi/audio/library/music/
ln -s ../../library/music/song.flac suzumushi/audio/playlists/focus/song.flac

suzumushi --root ./suzumushi
-> scans suzumushi/audio
-> user opens Playlists
-> user selects focus
-> queue starts in folder order
-> user presses / and searches an artist or song
-> user presses Space+/ for the global palette
-> lyrics appear if song.lrc exists
-> album art appears if the terminal supports images
-> a small visualizer breathes in Now Playing while audio plays
-> Waybar shows now playing
-> tmux can show now playing
-> Zellij can show now playing through a status plugin command
-> media keys control play/pause/next/previous
-> desktop notification appears on track change
```

Recommended app layout:

```txt
suzumushi/
|-- config.toml
|-- audio/
|   |-- library/
|   |   |-- music/
|   |   |   |-- artist/
|   |   |   |   |-- album/
|   |   |   |   |   |-- 01 song.flac
|   |   |   |   |   |-- 01 song.lrc
|   |   |-- podcasts/
|   |   |   |-- example-show/
|   |   |   |   |-- 001 episode.mp3
|   |   |-- audio-series/
|   |       |-- example-series/
|   |           |-- 01 episode.ogg
|   |-- playlists/
|   |   |-- demo/
|   |   |   |-- README.txt
|   |   |-- focus/
|   |   |   |-- 01 song.flac -> ../../library/music/artist/album/01 song.flac
|   |   |   |-- 02 copied-song.mp3
|   |   |-- night-drive/
|   |   |   |-- 01 intro.ogg
|   |   |   |-- 02 neon-rain.flac
|   |-- lyrics/
|       |-- README.txt
|-- state/
|   |-- now-playing.json
|   |-- artwork/
|-- logs/
|-- backups/
|   |-- tag-edits/
|   |-- lyric-edits/
```

Do not build in v1:

- Streaming service integration
- Spotify, Apple Music, YouTube Music, or any other service account connection
- Podcast/RSS subscriptions or automatic episode downloading
- Online lyric fetching
- Accounts
- Recommendations
- Cloud sync
- A background daemon that scans random system folders
- A database-heavy music manager in v1
- Remote control over the network
- Embedded album art editing
- Full-screen visualizer modes, plugin visual effects, or flashy FFT themes

Keep the product focused:

```txt
Local audio in. Calm listening out.
```

---

## Important Product Decisions

### Linux-only v1

Suzumushi should be Linux-only for v1.

Why:

- MPRIS is a Linux desktop integration standard
- Waybar is Linux/Wayland-focused
- Media key behavior can be implemented through MPRIS instead of direct keyboard grabs
- The app can use Linux file locking, atomic rename, and D-Bus integrations without carrying cross-platform compatibility work
- Packaging and QA stay smaller

This does not mean the Rust code should be carelessly platform-specific everywhere. Keep platform boundaries isolated in modules like `desktop/`, `status/`, and `paths/`, but do not spend v1 effort supporting Windows or macOS. V1 has no control socket or network/local IPC protocol beyond the desktop D-Bus integration and bounded read-only status file.

The v1 prebuilt baseline is `x86_64-unknown-linux-gnu`, generic x86_64 ISA,
glibc 2.35 or newer, and Linux 5.15 or newer. The audio pipeline decision records the exact dynamic
audio/DSP/image libraries or proves they are bundled appropriately; the oldest
supported clean-host image is a required QA target. Writable app-owned roots
must be on a local filesystem that passes the descriptor-relative no-follow,
same-parent `renameat2`, file/directory `fsync`, and lock semantics probe.
Unsupported NFS/FUSE/other filesystems may remain read/play-only with an explicit
diagnostic but never receive a weaker mutation path. The AUR ships only
`suzumushi-bin` from the verified prebuilt release artifact; there is no
source-building `suzumushi` AUR package.

### Wayland support contract

Suzumushi is Wayland-supported with documented integration fallbacks; it does
not claim that every optional desktop feature works in every compositor,
terminal, or minimal session. The core TUI, filesystem, queue, playback, lyrics,
tags, backups, search, tmux, and Zellij behavior is display-server independent
and must work in a terminal under pure Wayland with XWayland disabled.

- Waybar is a native Wayland integration and consumes only bounded status JSON.
- MPRIS/media keys and notifications use the user session D-Bus, not X11.
  Missing portals, media-key bindings, a notification daemon, or a D-Bus session
  produce bounded non-fatal diagnostics while playback remains usable.
- Audio is independent of Wayland. The audio pipeline decision must select and test
  the exact ALSA, PulseAudio, and/or PipeWire backend/features that ship; desktop
  session presence is not used as evidence that audio output works.
- Terminal artwork depends on Kitty/Sixel/iTerm2 protocol support and multiplexer
  passthrough, not Wayland itself. Unsupported or denied terminal queries use the
  bounded text/half-block fallback without delaying startup indefinitely.
- No feature silently falls back to an X11-only path when `DISPLAY` is absent.

Release QA includes pure-Wayland sessions without XWayland on the exact
versioned GNOME, KDE Plasma, and wlroots combinations in `docs/support-matrix.md`, with its named terminals,
PipeWire/PulseAudio behavior, MPRIS/media keys, notifications, Waybar, tmux,
Zellij, and image-protocol success/fallback cases. The matrix records compositor,
desktop/session services, terminal, audio backend, and every unavailable or
degraded integration.

### App-owned root, not `~/Music`

Suzumushi should never silently use the computer's original music folder.

Default root behavior:

```txt
suzumushi --root ./suzumushi
```

Bare command behavior:

```txt
suzumushi
```

Rules:

- If `--root <path>` is supplied, use it
- Else if `SUZUMUSHI_ROOT` is set, use that root
- Else if `./suzumushi/audio/` exists, use `./suzumushi`
- Else fail clearly and suggest `suzumushi init ./suzumushi`
- Do not fall back to `~/Music`
- Do not scan home directories automatically

This precedence is shared by TUI, diagnostics, backup, and read-only status
commands. Tests cover every source alone, every conflicting combination, an
invalid higher-priority root, relative and canonical paths, and the final error;
an invalid selected source is reported rather than silently falling through.

This keeps the app honest. If the user wants music from another place, they can copy files, create symlinks, or pass an explicit root.

### Filesystem-first playlists

Every folder under `suzumushi/audio/playlists/` is a playlist.

Example:

```txt
suzumushi/audio/playlists/focus/
suzumushi/audio/playlists/sleep/
suzumushi/audio/playlists/rainy-day/
```

Playlist rules:

- Child folders of `playlists/` become named playlists
- Audio files inside a playlist folder become playlist entries
- Copied files are supported
- Symlinked files are supported
- Broken symlinks are shown as errors, not crashes
- Symlinked directories are ignored in v1
- Natural filename order is the default order
- Hidden files are ignored unless they are sidecar `.lrc` files for visible audio

Do not add a separate playlist file format in v1. Folder playlists are the feature.

### Natural order first, shuffle as a queue

Folder order should be predictable.

Default order:

```txt
01 intro.flac
02 morning.mp3
03 night.ogg
10 finale.flac
```

Use natural sorting so `10` comes after `9`, not after `1`.

Natural order must be deterministic across runs: compare normalized display
segments using the documented case/number rule, then break ties with the exact
root-relative path bytes available on Linux. Locale, hash-map iteration order,
scan completion order, and filesystem enumeration order must not affect the
queue.

The v1 comparator is byte-defined rather than locale-defined. For each Linux
path component, split raw `OsStr` bytes into maximal ASCII-digit and non-digit
runs. Numeric runs compare by significant digit count, then significant digits,
then leading-zero count and original bytes. Non-digit runs compare with ASCII
letters folded to lowercase, then original bytes. If every component key ties,
compare the complete root-relative raw path bytes. Valid UTF-8 affects display
and search normalization only, never queue ordering; invalid UTF-8 is rendered
with a reversible safe escape and remains sortable. Versioned fixed vectors
cover empty names, `1/01/001/2/10`, ASCII case ties, Unicode bytes, invalid UTF-8,
duplicate display names, and nested paths independently of the implementation.

Shuffle should not pick a random next song every time. It should create a queue order.

Why:

- `next` and `previous` stay meaningful
- The user can inspect what is coming
- Tests are deterministic when the RNG seed is injected
- Repeat behavior is easier to reason about

Recommended shuffle model:

```txt
ordered track list -> seeded shuffle -> playback queue
```

### Local `.lrc` synced lyrics in v1

Suzumushi should support synced lyrics in v1, but only from local files.

Supported v1 format:

```txt
[00:12.40] First lyric line
[00:18.10] Second lyric line
[00:24.80] Third lyric line
```

Lookup order:

```txt
1. Same directory as the playlist entry: song.flac -> song.lrc
2. If playlist entry is a symlink, same directory as the canonical target
3. Mirrored path under suzumushi/audio/lyrics/
4. Unique filename-stem match under suzumushi/audio/lyrics/
```

Shared lyrics examples:

```txt
suzumushi/audio/library/music/lamp/album/01 song.flac
suzumushi/audio/lyrics/library/music/lamp/album/01 song.lrc

suzumushi/audio/playlists/focus/01 song.flac
suzumushi/audio/lyrics/playlists/focus/01 song.lrc
```

Filename-stem fallback should be conservative. If multiple `song.lrc` files exist in `audio/lyrics/`, Suzumushi should report an ambiguous lyrics warning instead of guessing.

No online lyric fetching in v1.

Why:

- Avoid copyright and licensing ambiguity
- Avoid API keys
- Avoid brittle scraping
- Avoid network failure in the core music path
- Keep v1 fully local and predictable

Lyrics should follow actual playback position. Speed changes, pause, seek, next, and previous must update the active lyric line from the same playback state used by the progress bar.

Lyrics should also be editable in v1.

Local lyric editing rules:

- Edit only local `.lrc` files
- If no `.lrc` exists, offer to create a sidecar file beside the audio file when possible
- If the track comes from a symlink, show whether the sidecar will be created beside the playlist entry or canonical target
- If lyrics came from `audio/lyrics/`, edit that shared file explicitly
- Validate timestamps before saving
- Preview parse errors before writing
- Write through a temporary file and atomic rename
- Back up the previous `.lrc` under `suzumushi/backups/lyric-edits/`
- Do not fetch, scrape, or generate lyrics online in v1

The lyric editor can be simple: timestamped lines, search, insert line, delete line, and save. It does not need to become a full text editor, but it must protect the file from malformed writes.

### MPRIS/media keys are v1

Suzumushi should expose a Linux MPRIS service in v1.

Service name:

```txt
org.mpris.MediaPlayer2.suzumushi
```

MPRIS should support:

```txt
Play
Pause
PlayPause
Stop
Next
Previous
Seek
SetPosition if reliable
Metadata updates
PlaybackStatus updates
Volume updates if supported cleanly
Album art URL when available
```

Why:

- Desktop media keys should work without Suzumushi grabbing global keys
- `playerctl play-pause` should control Suzumushi
- Wayland desktops already understand MPRIS
- The app can show up in media widgets later without separate integrations

Implementation recommendation:

- Start with `souvlaki` for Linux media controls and metadata if it covers the needed MPRIS behavior
- If it blocks correct Linux behavior, use `zbus` directly against the freedesktop MPRIS specification
- Do not shell out to `playerctl` from Suzumushi

If no D-Bus session is available, Suzumushi should continue playing music and show a non-fatal warning in the status panel.

The fixed bus name gives v1 one global player identity. In addition to the
per-canonical-root writer lock, mutable TUI startup acquires one global
single-active-TUI lock for the OS user before registering MPRIS on the current
session bus. `$XDG_RUNTIME_DIR` is per-user and shared by concurrent login
sessions, so v1 intentionally allows only one mutable TUI per UID, even when
another session or root is named. Bounded `waybar`, `tmux`, `zellij`, `diagnose`,
and backup status/list commands remain concurrent. Stale global-lock recovery
verifies process identity and is covered by same-session and cross-session tests.

### Desktop notifications are v1

Suzumushi should send a desktop notification on track change.

Notification content:

```txt
summary: Now playing
body: Artist - Title\nAlbum
image: cached album art path, when available
app name: Suzumushi
timeout: short, for example 4-6 seconds
```

Rules:

- Notifications are enabled by default on desktop sessions
- Notifications must be configurable off
- Do not notify repeatedly for pause, seek, speed, or progress updates
- Do not spam notifications during fast next/previous key repeats
- If the notification daemon is missing, show one non-fatal warning and keep playing
- Bound summary/body bytes, remove NUL/control characters, and escape notification markup according to the selected API/daemon contract; metadata is never treated as trusted markup

### Album art viewing is v1

Suzumushi should read and display album art when available.

Lookup order:

```txt
1. Embedded artwork from audio tags
2. cover.jpg / cover.png beside the audio file
3. folder.jpg / folder.png beside the audio file
4. If playlist entry is a symlink, check beside canonical target
```

Album art outputs:

- TUI panel through terminal image protocols when supported
- MPRIS `mpris:artUrl`
- Desktop notification image
- Text fallback when the terminal cannot show images

Rules:

- Cache decoded/extracted artwork under `suzumushi/state/artwork/`
- Bound artwork dimensions and decoded bytes
- Never decode images on the render path
- Fall back gracefully when image protocols are unsupported
- Do not edit embedded album art in v1

### Waybar widget is v1

Waybar integration should be a small command, not a second app.

Command:

```txt
suzumushi waybar --root /absolute/path/to/suzumushi
```

Output should be one-line JSON for Waybar custom modules:

```json
{"text":"Artist - Song","tooltip":"Album\n02:14 / 04:02\nSuzumushi","class":["playing"],"percentage":55}
```

Suggested Waybar config:

```jsonc
"custom/suzumushi": {
  "exec": "suzumushi waybar --root /home/user/path/to/suzumushi",
  "return-type": "json",
  "interval": 1,
  "format": "{text}",
  "escape": true,
  "tooltip": true
}
```

The Waybar command should read bounded `suzumushi/state/now-playing.json`, derive Waybar JSON, and exit quickly. It should not start the TUI, scan music, or initialize audio.

### Tmux status output is v1

Suzumushi should expose a small command for tmux status bars.

Command:

```txt
suzumushi tmux --root /absolute/path/to/suzumushi
```

Example output:

```txt
♪ Lamp - Yume Utsutsu 01:42/04:05
```

Rules:

- Read bounded, versioned `suzumushi/state/now-playing.json`
- Exit quickly
- Never start the TUI
- Never initialize audio
- Support a compact stopped state
- Keep output single-line and terminal-control-character safe

### Zellij status output is v1

Suzumushi should expose a small command for Zellij status bar plugins, especially `zjstatus`.

Command:

```txt
suzumushi zellij --root /absolute/path/to/suzumushi
```

Example output:

```txt
♪ Lamp - Yume Utsutsu 01:42/04:05
```

Rules:

- Read bounded, versioned `suzumushi/state/now-playing.json`
- Exit quickly
- Never start the TUI
- Never initialize audio
- Support a compact stopped state
- Keep output single-line and terminal-control-character safe
- Document `zjstatus` as the recommended v1 path
- Do not ship a custom Zellij WASM plugin in v1

### Tag editing is v1, but safe by default

Tag editing mutates user music files. Treat it as a safety-sensitive feature.

V1 editable fields:

```txt
title
artist
album
album_artist
track_number
disc_number
genre
year
comment
```

Defer:

```txt
embedded album art editing
automatic tag lookup
filename rewriting
replaygain editing
```

Safety rules:

- Show the exact file path before saving
- Show whether the selected playlist entry is a copy or symlink
- Resolve symlinks and show the canonical target before editing
- Refuse to edit any file currently loaded/open by the audio worker, including paused playback, until an unload acknowledgment
- Use a verified full-file backup by default before every write, deduplicating identical backup content within the budget
- Use verified full-file backups only in v1
- Keep tag-edit backups under `suzumushi/backups/tag-edits/`
- Enforce a configurable backup budget
- Fail closed if backup creation fails
- Never write tags from a preview screen without explicit save confirmation
- Sanitize control characters from displayed tag values
- Batch edits must preview every affected file and field before writing
- Batch edits must use per-file backups
- Batch edits must have a configurable max batch size

Full-file backup is the sole v1 mode. User music files matter. The swamp rule is simple: no verified full-file backup, no write.

Backups are never deleted automatically. Provide `suzumushi backups list`,
`verify`, `restore`, and explicit `prune` commands before any lyric or tag editing ships. A
full budget causes a clear fail-closed error that points to those commands;
pruning requires a preview and confirmation. Backup manifests include source
and canonical paths, stable file identity, size, modification time, content hash,
backup hash, tool version, mutation kind/format, journal state, and timestamp.

### No heavy music database in v1

Do not add SQLite just to list songs.

V1 should rebuild an in-memory index from `suzumushi/audio/` on startup and manual rescan.

Why:

- The folder is the source of truth
- Scanning is easier to test than cache invalidation
- A stale database would create confusing behavior
- Tag editing, symlinks, and playlist folders are already enough complexity

Add persistent indexing only if startup scan becomes too slow on real libraries.

### Manual rescan in v1

Users should be able to update folders outside the app.

V1 behavior:

```txt
User edits suzumushi/audio/ manually
User presses R in Suzumushi
Suzumushi rescans and updates library/playlists
```

Do not use live inotify watching in v1. It is useful later, but MPRIS, lyrics, tag editing, and audio controls are already enough for the first release.

---

## Controls

### Global

```txt
q          quit
?          help
Tab        next panel
Shift+Tab  previous panel
h/j/k/l    move in focused panel
Enter      open/select
Esc        close modal or return to previous focus
R          rescan suzumushi/audio
```

`Space` doubles as a leader key for chords like `Space+/` and `Space+l`. When
`Space` is pressed, Suzumushi waits a bounded, configurable timeout (default
250 ms, `leader_timeout_ms`) for a second chord key. If one arrives in time,
the chord fires; if the timeout expires with no second key, play/pause fires.
This intentional chord-disambiguation delay is tested with an injected clock so
`Space` fires at the configured timeout and a valid chord arriving before it
wins. This is the only timing-based disambiguation rule. Every other
overloaded key resolves by focused panel alone and never by timing: `Enter`,
`e`, `d`, `Tab`, and `Ctrl+s` each mean exactly one thing per panel, and no key
ever performs two actions at once.

Enter is context-sensitive per panel: on a container node (a library or
playlist folder), Enter descends into it, exactly like `l`. On a leaf track,
playlist, or search result, Enter prompts replace vs append. `l` also descends
into containers, so pure navigation never triggers a prompt.

### Playback

```txt
Space      play / pause
n          next track
p          previous track
Right      seek forward 5 seconds
Left       seek backward 5 seconds
Shift+Right seek forward 30 seconds
Shift+Left  seek backward 30 seconds
+          volume up
-          volume down
m          mute / unmute
<          speed down
>          speed up
=          reset speed to 1.0x
s          shuffle on / off
r          repeat off / one / queue
```

### Library And Queue

```txt
/          open audio search
Space+/    open global palette
g g        first item
G          last item
d          remove selected queue item
c          clear queue after confirmation
Enter      descend into selected folder; prompt replace vs append on song/playlist
a          append selected song/folder/playlist to queue
```

### Search And Palette

```txt
/          audio search: creators, titles, albums/series, playlists, filenames, paths
Space+/    global palette: audio search plus commands
Esc        close search or palette
Enter      select result, then prompt replace vs append when result is music
Tab        next result group
Shift+Tab  previous result group
Ctrl+n     next result
Ctrl+p     previous result
```

### Lyrics

```txt
Space+l    focus lyrics panel
L          toggle lyrics panel size
e          edit current lyric file when lyrics panel is focused
Ctrl+s     save lyric edit after validation
```

### Tags

```txt
i          show file and tag info
e          edit selected song tags
B          batch edit marked song tags
v          mark / unmark selected song for batch edit
Ctrl+s     save tag edit after confirmation
Esc        cancel tag edit
```

### Artwork

```txt
A          toggle album art panel
```

### Visualizer

```txt
V          toggle mini visualizer
```

Every action should have a keyboard path. Mouse support can be added later, but it is not required for v1.

---

## The Stack

| Concern | Choice | Why |
|---|---|---|
| Language | Rust 1.97.1 current-stable development/release baseline; Rust 1.95.0 MSRV, edition 2024 | Safe local systems code, strong state modeling, fast TUI apps, and explicit current-stable/MSRV contracts |
| TUI | `ratatui` | Mature terminal layout and widgets |
| Terminal backend | `crossterm` | Linux terminal input/output with event stream support |
| Async runtime | `tokio` with only required features | UI ticks, scanning tasks, audio commands, and MPRIS integration without the compile and supply-chain cost of `full` |
| Audio output | `rodio` if the focused playback work proves the required source/seek/device contract | Output device integration; Rodio's native speed control is not pitch-preserving and is not the v1 speed implementation |
| Audio decode/control path | One pipeline chosen when playback is implemented, using `rodio` decoding or `symphonia` | Decode into bounded PCM that can feed time-stretch, the visualizer tap, position accounting, and output without a second decode |
| Pitch-preserving speed | `signalsmith-stretch` or equivalent | Time-stretch without changing pitch for `0.5x` through `2.0x` playback |
| Audio visualizer | In-house RMS/peak sampling from decoded PCM | Small calm TUI level meter without FFT complexity in v1 |
| Tags | `lofty` | Read and write metadata across common audio formats |
| Desktop media controls | `souvlaki` first, `zbus` direct if needed | MPRIS/media keys on Linux without shelling out |
| Desktop notifications | `notify-rust` | Freedesktop notifications with optional image path |
| Terminal images | `ratatui-image` + `image` | Album art through Kitty/Sixel/iTerm2 protocols with fallback |
| File walking | App-owned descriptor-relative Linux walker using `rustix`; `openat2` restrictions when available | One bounded traversal rooted in a pinned directory descriptor, with no pathname re-open or parent-substitution gap |
| Sorting | Small app-owned byte-defined natural comparator | Deterministic Linux filename ordering, including non-UTF-8 names, without locale or crate-semantic ambiguity |
| Search matching | `nucleo-matcher` | Fast fuzzy matching over the in-memory library index |
| Serialization | `serde`, `serde_json`, `toml` | Config, state files, Waybar JSON, status output state |
| CLI | `clap` | `init`, `waybar`, `tmux`, `zellij`, root flags, diagnostic commands |
| Time | `time` | Timestamps, state files, logs |
| Randomness | `rand` | Deterministic shuffle with injectable seed in tests |
| File/root locks | `rustix` flock plus Linux file-identity checks | Reuse the descriptor-oriented platform boundary, enforce one writer per root, and avoid overlapping tag writes, backup races, and stale previews |
| Errors | `thiserror` + `anyhow` | Typed internal errors plus app-level context |
| Logging | `tracing` + `tracing-subscriber` + `tracing-appender` feeding an app-owned rotating writer | File logging without corrupting the TUI; `tracing-appender` owns bounded queue/drop-or-backpressure behavior while the app-owned writer supplies byte rotation and retained-file limits |
| Unicode display | `unicode-width` | Correct width for titles, artists, and lyrics |
| Testing | `tempfile`, `insta`, `proptest`, `assert_cmd` | Temporary music roots, reviewed UI snapshots, property tests, CLI behavior tests |
| Release packaging | one GitHub Actions release workflow | Linux archives and checksums from version tags; crates.io and package-manager updates remain explicit release work |

### Dependency inventory

This is a non-executable planning inventory, not a setup script or permission to
add every crate at bootstrap. Project setup uses the standard library wherever
possible. After the relevant ADR names a production choice, each phase adds only
the dependency it needs, pins the exact version and exact minimal feature set in
`Cargo.toml`, updates committed `Cargo.lock`, records license/native/unsafe
implications, and removes rejected spike dependencies. `Cargo.toml` and
`Cargo.lock` are authoritative; this table is reconciled whenever either changes.

| First need | Candidate inventory | Intended minimal features to decide and pin in ADR |
|---|---|---|
| Audio playback | `rodio` or `symphonia`; `signalsmith-stretch` or selected equivalent | Only proven codec/output/stretch features; no default feature set accepted without review |
| Project setup/CLI | `clap = 4.6.6` | Pinned with default features disabled and only `std`, `help`, and `usage`; MIT OR Apache-2.0, no native/FFI boundary, and reviewed internal unsafe only in the resolved `clap_lex` and `anstyle` dependencies |
| Foundation/errors/logging | `thiserror`, `anyhow`, `tracing`, `tracing-subscriber`, `tracing-appender` | Bounded nonblocking delivery into app-owned rotating file logging and only required formatting/registry features when first used |
| TUI/runtime | `ratatui`, `crossterm`, `tokio`, `unicode-width` | Crossterm event stream; Tokio runtime, macros, sync, time, signal, and fs only when used |
| Root/scan/config model | `rustix = 1.1.4`, `lofty = 0.24.0`, `serde = 1.0.229`, `serde_json = 1.0.151`, `toml = 1.1.4`, `thiserror = 2.0.20`, app-owned natural comparator | Exact minimal features in `Cargo.toml`; Lofty has defaults disabled and runs only behind the bounded helper contract in `docs/parser-isolation.md`; no pathname walker or sorting dependency |
| Later search/queue model | `nucleo-matcher`, `rand` | Select exact versions and minimal features only when search and shuffle behavior arrive |
| Tags/artwork/desktop | `lofty`, `image`, `ratatui-image`, `souvlaki` or `zbus`, `notify-rust`, `time` | Exact supported formats/protocols/backends selected by their ADRs |
| Current tests/fuzz | `tempfile = 3.27.0`, `cargo-fuzz = 0.13.2`, `libfuzzer-sys = 0.4.13` | Temporary-root behavior tests plus four bounded untrusted-input targets; later UI/property/CLI helpers remain unselected |

The initial package defines no Cargo application features. The single audio path
selected during audio playback work is compiled unconditionally, is the default, and is the sole
v1 release set. Future application features require an ADR with a supported
feature matrix; CI, QA, and packaging must name and use the exact
release set rather than opportunistically testing different combinations.

Temporary dependency exception: `RUSTSEC-2024-0436` marks transitive
`paste 1.0.15` from `lofty 0.24.0` as unmaintained without reporting a
vulnerability. The maintainer owns the narrow audit/deny exception through
2026-11-09; remove it earlier when Lofty drops the crate, a maintained parser is
selected, or a vulnerability is reported. No other advisory warning is allowed.

Use strict lints from the start:

```toml
[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
pedantic = { level = "warn", priority = -1 }
```

Warning denial is a CI mechanism, not a `[lints]` entry. Current-stable Cargo
1.97.1 build/test tasks set `CARGO_BUILD_WARNINGS=deny`, Clippy
passes `-- -D warnings`, and the Rust 1.95.0 MSRV task uses the compatible
`RUSTFLAGS="-D warnings"` fallback. Local builds stay warn-level so new compiler
warnings do not break developers mid-change.

Toolchain policy:

- Use Rust edition `2024`. `package.rust-version = "1.95.0"` is the supported MSRV, not the development or release compiler. The authoritative `mise.toml` pins the normal development and release baseline to Rust `1.97.1`.
- Project setup defines `fmt`, `clippy`, `test`, `msrv`, and `ci` in `mise.toml`. Add `security`, fuzz, fault, release, and QA tasks only when the corresponding dependency, parser, mutation, or release work arrives. The `msrv` task invokes Rust 1.95.0 explicitly through `mise exec rust@1.95.0 --`; all normal tasks use Rust 1.97.1. Raising either baseline or MSRV requires a recorded decision and `mise.toml` update; `package.rust-version` changes only when MSRV changes.
- License the package as `Apache-2.0`, set `package.license = "Apache-2.0"`, commit the canonical Apache-2.0 `LICENSE` text, and apply `SPDX-License-Identifier: Apache-2.0` headers to every source file. Pin this before the first public commit: code released under one license stays under it for everyone who received it, so the choice is effectively permanent once published. Changing it requires an ADR, a dependency/license review, and the agreement of every external contributor.
- Commit `Cargo.lock`; CI, packaging, and releases use `--locked`.
- Local development, CI, and releases use `mise run ...`; workflow YAML only checks out the repository, installs a pinned `mise`, and calls committed tasks.
- When third-party runtime dependencies arrive, add a focused `security` task using pinned `cargo deny check` and `cargo audit`; document any temporary advisory exception with an owner and expiry.
- Dependency updates are manual and must pass `mise run ci`; incompatible updates require a reviewed MSRV decision rather than silently raising it.

```toml
[tools]
rust = "1.97.1"

[tasks.msrv]
run = 'mise exec rust@1.95.0 -- cargo test --locked --workspace'

[tasks.ci]
run = [
  { task = "fmt" },
  { task = "clippy" },
  { task = "test" },
  { task = "msrv" },
]
```

---

## Architecture Overview

```txt
+-------------------------------------------------------------------+
| main.rs               - thin binary: startup and CLI args          |
| lib.rs                - library root for tests and fuzz            |
+-------------------------------------------------------------------+
| cli.rs                - init, tui, waybar, tmux, zellij, diagnostics |
| config.rs             - root config, defaults, limits              |
| paths.rs              - suzumushi root, music paths, state paths       |
| terminal.rs           - raw mode, alternate screen, cleanup         |
| app.rs                - top-level UI state and view routing         |
| event.rs              - terminal events, ticks, app events          |
| input.rs              - keybindings and action mapping              |
| command.rs            - app commands and confirmations              |
| model/                - tracks, playlists, queue, tags, lyrics      |
| scan/                 - music root scanner and symlink policy        |
| search/               - in-memory index, ranking, palette results    |
| audio/                - playback worker and audio commands          |
| dsp/                  - pitch-preserving speed/time-stretching       |
| visualizer/           - audio level frame reduction for the TUI       |
| lyrics/               - LRC parser and active line resolver          |
| lyrics_editor/        - local LRC editing, validation, backups       |
| tags/                 - metadata read/edit/write/backups            |
| backups/              - mutation journal, backup/verify/restore/prune |
| artwork/              - artwork extraction, cache, terminal images   |
| desktop/              - MPRIS, media keys, notifications             |
| status/               - one state schema plus Waybar/tmux/Zellij rendering commands |
| ui/                   - layouts, widgets, views, themes, help        |
| errors.rs             - typed errors                                |
+-------------------------------------------------------------------+
```

## Runtime Model

- The UI loop owns `AppState`
- The UI loop is the sole owner of the queue, current `QueueItemId`, shuffle/repeat/history, `queue_generation`, and all queue mutations
- The audio worker owns only playback of the `QueueItemSnapshot` selected by the UI, plus the audio output device and active playback handle; it never advances or mutates the queue
- One root lock owns mutable root state, and one per-user global lock enforces a single active v1 TUI identity across all login sessions and roots. A second TUI for the same UID fails clearly even on another session or root; bounded read-only status commands do not take either writer lock. MPRIS is registered only on the winning process's current session bus.
- The scanner and supported local filesystem work run as cooperatively cancellable background tasks at documented entry/read/parser boundaries
- The search index is rebuilt from the latest complete scan result
- Search and palette state live in the UI loop and never block playback
- The artwork worker extracts and decodes album art off the render path
- Tag writes run in a blocking task and never on the UI thread
- Lyric writes validate and write atomically off the UI thread
- Pitch-preserving speed runs inside the owned audio path with bounded buffers
- Visualizer frames are derived from decoded Suzumushi playback samples and sent through a bounded, droppable channel
- MPRIS integration translates desktop commands into normal app actions
- Desktop notifications consume track-change events only
- Waybar, tmux, and Zellij commands read the same bounded, versioned `now-playing.json`, derive their output, and exit quickly
- Reliable bounded control channels carry accepted commands and state/ended/error acknowledgements; they are never dropped or displaced by telemetry. Reliability is implemented without cyclic waits: the UI and audio actor loops never await outbound capacity while they are the sole receiver for the opposite lane. Each loop owns a bounded reliable outbox and uses one `select`-style driver to continue draining inbound messages while publishing pending outbound work. A full outbox stops admission of new commands before state mutation and returns a bounded busy result; already accepted work retains its one terminal acknowledgement. External D-Bus calls use the same nonblocking admission rule.
- The real-time audio callback performs no blocking channel send, allocation, logging, D-Bus work, or lock acquisition. It writes only to the preallocated audio path and bounded nonblocking telemetry handoff proven by the selected audio design.
- A capacity-one latest-value channel carries coalescible position events, and a separate bounded channel carries droppable visualizer events
- Progress updates are capped to a low frequency, for example 4-10 Hz in the TUI; the persisted status projection is event-driven and renderers extrapolate playing position from a boot-clock sample, so there is no one-second disk heartbeat. Visualizer frames are capped separately, usually 8-12 Hz.
- Logs always go to files, never stdout while the TUI is active; `tracing-appender` owns the bounded queue/drop policy and the app-owned writer owns byte/count rotation, with dropped diagnostic records counted rather than blocking playback

Recommended event flow:

```txt
terminal input -> InputEvent -> AppAction
MPRIS command  -> AppAction
AppAction      -> UI queue mutation or AudioCommand / ScanRequest / SearchQuery / TagRequest / LyricRequest / ArtworkRequest
UI selection   -> AudioCommand::LoadTrack(QueueItemId snapshot)
worker event   -> AppEvent
audio samples  -> VisualizerFrame -> AppEvent
AppEvent       -> AppState update + redraw
scan complete  -> replace search index atomically in AppState
state update   -> versioned now-playing.json atomic write
track change   -> notification event
tick/resize    -> redraw
```

Do not use `Arc<Mutex<AppState>>` as the main architecture. Keep UI state owned by the app loop and move data through messages.

Every spawned worker has one owner, a bounded input/output channel, cancellation,
and an awaited shutdown path. Cancellation is a request, not a promise to interrupt an arbitrary blocking kernel/filesystem/library call: v1 supports only scoped local filesystem and cooperative work at documented cancellation points. `shutdown_timeout_ms` bounds waiting for cooperative workers, not a blocking operation already inside an uninterruptible call. If a product requirement needs a hard wall-clock bound for an uncooperative parser or helper, run it in an isolated helper process with bounded input/output and terminate/reap that helper; do not claim a Tokio task can be forcibly cancelled safely. Queue mutations carry `expected_queue_generation`;
active audio commands/events carry `QueueItemId` and `playback_generation`, with
queue generation retained only as creation/debug context. Scanner and dependent
read-only media work carry `scan_generation`. Stale scan, artwork lookup, lyric
lookup, state projection, audio-position, and visualizer events are discarded.
Committed lyric/tag/restore mutation acknowledgments are never discarded by a
new scan generation: they are reliable, keyed by operation ID, and force a fresh
reconciliation scan after their terminal journal disposition. Reliable
control/state events are handled in order. Reliable state acknowledgements are
never rejected because telemetry from another channel arrived first; position
and visualizer lanes use independent sequence numbers and may coalesce or drop.
Operational errors are reported and recovered where possible. Stale external
messages, malformed persisted journal phases, and other input-derived ordering
violations are typed operational errors with diagnostics and safe recovery,
never assertions. Impossible transitions constructed solely by trusted
in-process code are made unrepresentable where practical and use release
`assert!` or a documented fatal programmer-error path when continuing could
corrupt queue, playback, or mutation state.
The terminal guard restores state on normal exit and unwinding panic, but cannot
guarantee restoration after abort, `SIGKILL`, or power loss; recovery steps are
documented. `debug_assert!` is reserved for cheap, non-correctness-bearing
expectations and never substitutes for a critical release invariant. Untrusted media, tag, and lyric input must never reach
an assertion; it is validated and rejected as an operational error. Assertions
never contain side effects.

The visualizer tap is intentionally after volume and mute: muted output renders silence, and reduced volume lowers displayed levels. This is one product contract shared by the pipeline diagram, tests, and UI; moving the tap requires an ADR because it changes user-visible semantics.

---

## Directory Model

Root discovery order:

```txt
1. --root <path>
2. SUZUMUSHI_ROOT environment variable
3. ./suzumushi if ./suzumushi/audio exists
4. clear error with init instructions
```

`suzumushi init ./suzumushi` should create:

```txt
suzumushi/
|-- config.toml
|-- audio/
|   |-- library/
|   |-- playlists/
|   |   |-- demo/
|   |   |   |-- README.txt
|   |-- lyrics/
|   |   |-- README.txt
|-- state/
|-- logs/
|-- backups/
|   |-- tag-edits/
|   |-- lyric-edits/
```

Path rules:

- `audio/library/` is optional but recommended and may contain any user-defined hierarchy
- `audio/playlists/` is required for playlist discovery
- `audio/playlists/demo/README.txt` explains how to copy or symlink audio files into playlist folders
- `audio/lyrics/README.txt` explains sidecar and shared `.lrc` lookup rules
- `state/` is app-owned and can be recreated
- `state/now-playing.json` is the single versioned source for all status commands; derived Waybar/tmux/Zellij files do not exist
- `state/artwork/` is app-owned cache for extracted artwork
- `logs/` is app-owned
- `backups/` is app-owned and must not be silently deleted
- `state/`, `backups/`, and `logs/` are private current-UID data: `init` creates directories as `0700` and new state, journal, manifest, backup, and log files as `0600`. Existing app-owned directories are owner/no-follow verified before use and unsafe ownership or group/other access fails with a specific remediation message rather than silently inheriting exposure. Backups never receive broader access than their source and default to `0600`.
- Audio symlinks may point outside the root, but the UI must show that clearly
- External symlink targets are playback/read-only in v1. Lyric, tag, restore, backup, cache, state, log, and journal mutation targets must resolve beneath the canonical root; an external target is refused before backup or write.
- Symlinked directories are ignored in v1
- Canonical paths are used for deduplication
- Canonical paths deduplicate media assets only. Playlist entries remain distinct so one asset can appear in the library and multiple playlists with different names, order, and local sidecars.
- Display paths should be relative to the Suzumushi root when possible
- `init` canonicalizes the root, refuses to overwrite non-Suzumushi/non-empty destinations without explicit confirmation, and does not follow a symlinked root silently
- Mutable TUI mode acquires a root-level single-writer lock; stale-lock recovery verifies process identity rather than deleting a lock blindly
- Mutable TUI mode also acquires the per-user global active-TUI lock required by the fixed v1 identity; status commands remain concurrent
- The per-user active-TUI lock lives only under a verified current-UID-owned `$XDG_RUNTIME_DIR/suzumushi/` directory (`0700`) with a `0600` no-follow lock file. The [XDG base-directory contract](https://specifications.freedesktop.org/basedir/0.8/) makes this runtime directory per-user, and normal systemd sessions share it across concurrent login sessions for one UID, so this contract intentionally permits one mutable Suzumushi TUI per OS user, not one per login session. `/run/user/<uid>` may be used only when it is the verified runtime directory for that UID; there is no shared-`/tmp` fallback. If no secure runtime directory exists, mutable TUI/MPRIS startup fails closed while read-only diagnostics remain available.

Every mutable path follows the same descriptor-relative contract. Open the canonical root once as a
pinned directory descriptor; traverse each app-owned parent descriptor-relative
with no-follow checks, reject symlinked/substituted components, and retain the
verified parent handle through temporary creation, rename, and any required
durability sync. The
final existing target is opened `O_NOFOLLOW`, verified to be a regular file and
rechecked by device/inode/link-count before use. New targets are created only
relative to the verified parent. Replacement uses same-parent `renameat2` where
available, with `RENAME_NOREPLACE` for absence-required creation; unsupported
kernel/filesystem semantics or a required atomic operation that cannot be
provided returns explicit `unsupported` or `non-atomic` failure. No path-based
fallback, cross-directory rename, or best-effort external-target mutation is
allowed in v1. Reconstructible `now-playing.json` status projection writes use
the same descriptor, ownership, no-follow, and atomic-visibility rules but
deliberately skip file and parent `fsync`; durable mutation journals, backups,
manifests, tag/lyric replacements, and recovery state retain the full sync
contract.

Supported v1 audio extensions:

```txt
mp3
flac
wav
ogg
oga
m4a
aac
```

Extension detection is only the first filter. Decoder errors must still be handled cleanly.

Extensions are discovery hints, not codec guarantees. Before advertising playback support, commit a
decode compatibility matrix naming the tested container/codec combinations for
MP3, FLAC, WAV, Ogg Vorbis/Opus as applicable, M4A/AAC, and OGA. A combination
is supported only after a licensed fixture passes decode, seek, duration,
position, end-of-track, speed, and error-recovery tests through the chosen
production pipeline. Manual QA covers every advertised combination; unsupported
codecs produce a specific non-crashing diagnostic rather than being implied by
the filename extension.

---

## Config Model

Config path:

```txt
suzumushi/config.toml
```

Example config:

```toml
theme = "terminal" # terminal palette; use "mono" to disable color
default_view = "playlists"
default_volume = 0.70
volume_step = 0.05
speed_min = 0.50
speed_max = 2.00
speed_step = 0.25
seek_short_seconds = 5
seek_long_seconds = 30
shuffle = false
repeat = "off"
queue_max_items = 10000
queue_max_bytes = 50331648
queue_max_history_items = 1000

[input]
leader_timeout_ms = 250

[scan]
max_files = 50000
max_entries = 100000
max_depth = 16
max_playlists = 2000
max_entries_per_playlist = 10000
max_symlink_resolutions = 50000
max_parser_attempts = 50000
cross_mounts = false
max_path_bytes = 4096
max_total_path_bytes = 25165824
max_metadata_field_bytes = 16384
max_total_metadata_bytes = 50331648
max_warning_bytes = 8388608
max_index_bytes = 117440512
follow_file_symlinks = true
follow_directory_symlinks = false
ignore_hidden_audio = true

[search]
enabled = true
max_results = 200
max_query_bytes = 4096
music_search_key = "/"
palette_key = "space slash"
metadata_weight = 3
filename_weight = 1

[lyrics]
enabled = true
editing = true
shared_lookup = true
max_lrc_bytes = 262144
max_lines = 5000
backup_budget_bytes = 268435456
backup_max_entries = 10000

[artwork]
enabled = true
terminal_images = true
max_decode_bytes = 33554432
max_cache_bytes = 33554432
max_source_bytes = 33554432
max_width_px = 4096
max_height_px = 4096
preferred_size_px = 512

[visualizer]
enabled = true
style = "suzu" # suzu or bars
bars = 16
height = 1
update_hz = 10
falloff = "soft"

[desktop]
mpris = true
notifications = true
notification_timeout_ms = 5000
status_write_coalesce_ms = 250

[runtime]
process_memory_budget_bytes = 402653184
command_channel_capacity = 32
event_channel_capacity = 64
position_channel_capacity = 1
visualizer_channel_capacity = 2
max_open_files = 64
max_blocking_jobs = 4
max_parser_helpers = 2
tokio_worker_threads = 2
audio_prefetch_frames = 16384
state_file_max_bytes = 1048576
status_text_max_bytes = 4096
shutdown_timeout_ms = 5000

[logging]
max_file_bytes = 10485760
max_files = 5
max_record_bytes = 16384
queue_capacity = 512
queue_max_bytes = 8388608
queue_full_policy = "drop_and_count"

[tags]
editing = true
batch_editing = true
backup_budget_bytes = 2147483648
backup_max_entries = 10000
max_batch_files = 200
max_field_bytes = 16384
refuse_loaded_audio_file = true

[mutation]
journal_record_max_bytes = 1048576
journal_max_entries = 10000
journal_max_total_bytes = 67108864
startup_recovery_max_records = 10000
```

Rules:

- Invalid config fails with a clear message before starting the TUI
- Missing config uses safe defaults after `suzumushi init`
- Runtime changes should be explicit, not hidden magic
- Editing has no backup bypass: an existing lyric requires a verified lyric backup, and every tag write requires its configured verified backup. If backup preflight or creation fails, editing fails closed.
- Visualizer config must be bounded so it cannot force excessive audio analysis or redraw work
- Config should not contain secrets in v1 because Suzumushi has no network integrations
- Reject zero, non-finite, overflowed, contradictory, unknown, or out-of-range values before workers start; there is no `unlimited` sentinel
- Queue expansions and mutations perform checked item-and-byte preflight against `queue_max_items` and `queue_max_bytes` before changing state; overflow or limit+1 leaves the queue and generations unchanged
- Playback history is bounded by `queue_max_history_items`: the oldest entries drop when the bound is reached, and `previous` walks only the retained history
- `leader_timeout_ms` and every other scalar use the exact compiled ranges below; validation never relies on a subjective "reasonable" threshold
- `status_write_coalesce_ms` bounds burst coalescing, not liveness. There is no periodic status heartbeat and no wall-clock age threshold. Status liveness uses boot ID, `CLOCK_BOOTTIME`, process start ticks, writer-instance identity, and the held per-user lock; renderers extrapolate playing position from the last acknowledged source-position sample.
- Queue, playlist, metadata, path, log, state, channel, PCM, artwork, lyric, tag, backup, and shutdown limits are mandatory from their first phase
- `process_memory_budget_bytes` is the authoritative 384 MiB app-owned allocation budget. It accounts checked reserved capacity for active and replacement indexes, queues, search results, parser buffers, decoded PCM/stretch/output buffers, channels, artwork decode, status/journal/backup workspaces, logs, and all app-owned model/container/string capacities. Allocations reserve before growth and release only after ownership ends.
- `max_index_bytes` is the inclusive 112 MiB per-index cap for assets, contextual entries, paths, metadata, playlists, search fields, warnings, vector/string capacities, and lookup tables. Old and replacement indexes reserve 224 MiB together. The reviewed default app-owned partition is 224 MiB indexes, 48 MiB queue, 32 MiB parser/DSP/channels, 32 MiB artwork decode, 16 MiB mutation workspaces, 16 MiB UI/state/logging, and 16 MiB unassigned headroom. Path, metadata, and warning totals are sub-budgets, not additional allowances.
- Logging uses both count and byte bounds plus app-owned size/count rotation. Formatting rejects or visibly truncates a diagnostic above `max_record_bytes` before enqueue; the worst-case 8 MiB queue capacity is reserved from the UI/state/logging sub-budget and process ledger up front. `tracing-appender` provides delivery but not byte rotation. The default queue-full policy drops and counts diagnostic records rather than blocking the UI/audio path; the dropped count is visible in diagnostics.
- `max_open_files`, `max_blocking_jobs`, `max_parser_helpers`, and worker-thread counts are hard ownership limits enforced before open/spawn. Blocking parser, artwork, backup, and mutation work uses bounded semaphores; diagnostics expose current/high-water counts, and limit-plus-one tests prove refusal without leaked descriptors or detached tasks.
- Use unit-bearing names and integer byte/frame counts at boundaries; floating-point volume/speed values are finite and clamped before entering the audio worker

### Compiled configuration ranges

These ranges are implementation constants and part of the v1 safety contract.
The example values above are defaults. Resource ceilings may be lowered but
never raised beyond this table; a narrower process-memory reservation can also
reject a value that is individually within range. Every field is tested at its
minimum, maximum, and both adjacent invalid values where representable.

| Config field(s) | Exact accepted range or invariant |
|---|---|
| `theme` | `"terminal"` or `"mono"` |
| `default_view` | `"library"`, `"playlists"`, or `"queue"` |
| `default_volume` | finite `0.0..=1.0` |
| `volume_step` | finite `0.01..=1.0` |
| `speed_min`, `speed_max` | finite `0.50..=2.00`, with `speed_min <= 1.0 <= speed_max` |
| `speed_step` | finite `0.01..=1.50` |
| `seek_short_seconds`, `seek_long_seconds` | `1..=3600`, with short `<=` long |
| `repeat` | `"off"`, `"one"`, or `"queue"` |
| `queue_max_items` | `1..=10_000` |
| `queue_max_bytes` | `1..=50_331_648` |
| `queue_max_history_items` | `1..=1_000` and `<= queue_max_items` |
| `leader_timeout_ms` | `50..=2_000` |
| `scan.max_files` | `1..=50_000` |
| `scan.max_entries` | `1..=100_000` and `>= scan.max_files` |
| `scan.max_depth` | `1..=16` |
| `scan.max_playlists` | `1..=2_000` |
| `scan.max_entries_per_playlist` | `1..=10_000` |
| `scan.max_symlink_resolutions`, `scan.max_parser_attempts` | each `1..=50_000` |
| `scan.max_path_bytes` | `1..=4_096` |
| `scan.max_total_path_bytes` | `1..=25_165_824` |
| `scan.max_metadata_field_bytes` | `1..=16_384` |
| `scan.max_total_metadata_bytes` | `1..=50_331_648` |
| `scan.max_warning_bytes` | `1..=8_388_608` |
| `scan.max_index_bytes` | `1..=117_440_512` inclusive of paths, metadata, warnings, lookup tables, and capacities |
| `search.max_results` | `1..=200` |
| `search.max_query_bytes` | `1..=4_096` |
| `search.metadata_weight`, `search.filename_weight` | each `0..=16`, with at least one non-zero |
| `lyrics.max_lrc_bytes` | `1..=262_144` |
| `lyrics.max_lines` | `1..=5_000` |
| `lyrics.backup_budget_bytes` | `1..=268_435_456` |
| `lyrics.backup_max_entries` | `1..=10_000` |
| `artwork.max_source_bytes`, `artwork.max_decode_bytes`, `artwork.max_cache_bytes` | each `1..=33_554_432` |
| `artwork.max_width_px`, `artwork.max_height_px` | each `1..=4_096`; checked pixel and decoded-byte products must also fit `max_decode_bytes` |
| `artwork.preferred_size_px` | `1..=min(max_width_px, max_height_px)` |
| `visualizer.bars` | `1..=32` |
| `visualizer.height` | `1..=4` |
| `visualizer.update_hz` | `1..=12` |
| `visualizer.style` | `"suzu"` or `"bars"` |
| `visualizer.falloff` | `"soft"` or `"none"` |
| `desktop.notification_timeout_ms` | `100..=60_000` |
| `desktop.status_write_coalesce_ms` | `50..=1_000` |
| `runtime.process_memory_budget_bytes` | `67_108_864..=402_653_184` |
| `runtime.command_channel_capacity` | `1..=32` |
| `runtime.event_channel_capacity` | `1..=64` |
| `runtime.position_channel_capacity` | exactly `1` |
| `runtime.visualizer_channel_capacity` | `1..=2` |
| `runtime.max_open_files` | `1..=64` |
| `runtime.max_blocking_jobs` | `1..=4` |
| `runtime.max_parser_helpers` | `1..=2` |
| `runtime.tokio_worker_threads` | `1..=2` |
| `runtime.audio_prefetch_frames` | `1_024..=16_384` |
| `runtime.state_file_max_bytes` | `1..=1_048_576` |
| `runtime.status_text_max_bytes` | `1..=4_096` |
| `runtime.shutdown_timeout_ms` | `100..=5_000`; committed mutations are excluded and remain awaited |
| `logging.max_file_bytes` | `1..=10_485_760` |
| `logging.max_files` | `1..=5` |
| `logging.max_record_bytes` | `1..=16_384` |
| `logging.queue_capacity` | `1..=512` |
| `logging.queue_max_bytes` | `1..=8_388_608` |
| `logging.queue_full_policy` | exactly `"drop_and_count"` in v1 |
| `tags.backup_budget_bytes` | `1..=2_147_483_648` |
| `tags.backup_max_entries` | `1..=10_000` |
| `tags.max_batch_files` | `1..=200` |
| `tags.max_field_bytes` | `1..=16_384` |
| `mutation.journal_record_max_bytes` | `1..=1_048_576` |
| `mutation.journal_max_entries`, `mutation.startup_recovery_max_records` | each `1..=10_000` |
| `mutation.journal_max_total_bytes` | `1..=67_108_864` |

Boolean fields accept only TOML booleans. Keybinding strings are UTF-8, at most
64 bytes, and must parse to exactly one supported chord. `deny_unknown_fields` is applied
to every config section so a misspelled safety limit cannot silently use a
default.

---

## Core Models

Suggested app-owned types:

```rust
pub struct MediaAsset {
    pub id: MediaAssetId,
    pub canonical_path: PathBuf,
    pub tags: TrackTags,
    pub duration: Option<Duration>,
    pub artwork: Option<ArtworkRef>,
    pub file_identity: FileIdentity,
}

pub struct TrackEntry {
    pub id: TrackEntryId,
    pub asset_id: MediaAssetId,
    pub display_path: PathBuf,
    pub source: TrackEntrySource,
    pub lyrics_path: Option<PathBuf>,
}

pub enum TrackEntrySource {
    LibraryFile { relative_path: PathBuf },
    PlaylistCopy { playlist: PlaylistId },
    PlaylistSymlink { playlist: PlaylistId, target: PathBuf },
}

pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub path: PathBuf,
    pub entries: Vec<TrackEntryId>,
}

pub struct QueuedTrack {
    pub id: QueueItemId,
    pub entry_id: TrackEntryId,
    pub asset_id: MediaAssetId,
    pub canonical_path: PathBuf,
    pub file_identity: FileIdentity,
    pub display: QueueDisplaySnapshot,
    pub source_context: TrackEntrySource,
}

pub struct Queue {
    pub ordered: Vec<QueuedTrack>,
    pub play_order: Vec<QueueItemId>,
    pub history: Vec<QueueItemId>,
    pub current: Option<QueueItemId>,
    pub mode: QueueMode,
    pub repeat: RepeatMode,
    pub generation: u64,
    pub accounted_bytes: usize,
}

pub enum QueueMode {
    Ordered,
    Shuffled { seed: u64 },
}

pub enum RepeatMode {
    Off,
    One,
    Queue,
}

pub struct ArtworkRef {
    pub source: ArtworkSource,
    pub cache_path: Option<PathBuf>,
}

pub enum ArtworkSource {
    Embedded,
    SidecarFile { path: PathBuf },
}

pub struct BatchTagEdit {
    pub assets: Vec<MediaAssetId>,
    pub changes: TagChanges,
}

pub struct QueueItemSnapshot {
    pub queue_item_id: QueueItemId,
    pub queue_generation_at_creation: u64,
    pub canonical_path: PathBuf,
    pub file_identity: FileIdentity,
    pub display: QueueDisplaySnapshot,
}

pub struct SearchIndex {
    pub entries: Vec<SearchEntry>,
    pub generation: u64,
}

pub struct SearchEntry {
    pub scan_generation: u64,
    pub target: SearchTarget,
    pub group: SearchGroup,
    pub display: String,
    pub fields: SearchFields,
}

pub struct SearchFields {
    pub metadata: Vec<String>,
    pub filename: String,
    pub relative_path: String,
}

pub enum SearchTarget {
    Track(TrackEntryId),
    Artist { name: String, entries: Vec<TrackEntryId> },
    Album { name: String, entries: Vec<TrackEntryId> },
    Playlist(PlaylistId),
    Command(AppCommand),
}

pub enum SearchGroup {
    Artists,
    Songs,
    Albums,
    Playlists,
    Commands,
}
```

Keep raw filesystem paths out of rendering as much as possible. The UI should render sanitized display strings derived from app-owned models.

Identity and rescan invariants:

- A `MediaAsset` is deduplicated by canonical path plus verified file identity; a `TrackEntry` preserves library/playlist membership and sidecar lookup context.
- The same asset may have many entries and may intentionally occur more than once in a playlist or queue.
- `PlaylistId` derives from the playlist's root-relative path. `TrackEntryId` derives from playlist identity plus root-relative entry path. IDs are stable only while those paths retain the same meaning.
- A queue stores bounded path, display, source-context, and file-identity snapshots, not borrowed scan-index entries. Rescan may remove an entry from browsing without corrupting the active queue; playback still revalidates the retained expected identity before opening.
- Every replace, append, remove, clear, reorder, shuffle, or repeat mutation is checked against queue item/byte bounds before commit and increments `queue_generation` exactly once. Rejected operations change nothing.
- The app/UI loop alone owns queue navigation and mutation. On user next/previous or a matching `TrackEnded`, it resolves repeat/shuffle/history and sends one `LoadTrack` snapshot by `QueueItemId`; the audio worker cannot request or infer another item.
- Queue actions use `expected_queue_generation` for optimistic mutation checks. Active playback events are accepted by matching `QueueItemId` plus `playback_generation`; `queue_generation_at_creation` is diagnostic only and a later append/remove/repeat/shuffle change does not invalidate events from the still-playing item.
- `play_order`, history, and current selection store stable `QueueItemId` values and resolve them through checked lookup. They never retain positional indexes into a mutable vector. Removing an item removes that ID from future play order and retained history without changing the meaning of any other duplicate.
- Shuffle keeps immutable item identities plus an explicit seeded `play_order` and history. Toggling shuffle preserves the current item, `previous` follows actual retained ID history, duplicates remain distinct, and toggling off restores natural future order without rewriting history.
- Browser and search actions carry the displayed `scan_generation`; selecting after an index replacement is rejected with a refresh prompt rather than resolving a reused path/ID in a newer scan.
- A successful in-app tag rewrite reopens and verifies the new file identity, then atomically updates every queued snapshot for that `MediaAssetId` as one queue mutation. An external identity change is never adopted silently; playback fails closed until a rescan and explicit queue replacement/reconciliation.

Current-item mutation contract:

| Queue action while playing/paused | Active playback | Queue/history result |
|---|---|---|
| Append or change repeat | Continues unchanged | Generation advances; active events remain valid by item/playback identity |
| Shuffle on/off | Continues unchanged and remains current | Future play order changes around the current ID; prior history is preserved |
| Remove a non-current item | Continues unchanged | Removed ID disappears from play order/history |
| Remove the current item | UI sends one generation-transitioning `Stop`, then loads the deterministic successor only after stop acknowledgment | Removed ID leaves queue/history; no later stale end event can advance twice |
| Clear | UI sends one generation-transitioning `Stop` and waits for acknowledgment | Queue, play order, history, and current ID become empty atomically |
| Replace | UI stops the current generation, atomically installs the replacement, then loads its first item if requested | Old IDs cannot be resolved after commit |
| Reorder | Current item continues | Only future order changes; identity and prior history remain stable |

---

## Scan Model

Scanner responsibilities:

- Perform exactly one descriptor-rooted traversal of `suzumushi/audio/` per scan request using the app-owned `rustix` walker; classify library, immediate playlist children, shared lyrics, sidecars, and artwork candidates during that traversal rather than walking any subtree again
- Identify supported audio files
- Resolve file symlinks
- Ignore directory symlinks
- Pair `.lrc` sidecars with audio files
- Index `suzumushi/audio/lyrics/` as a real v1 shared lyrics path
- Read lightweight tags needed for display
- Emit searchable metadata fields for artists, albums, songs, playlists, filenames, and paths
- Report broken files without crashing the scan
- Return a complete replacement index to the UI

“Descriptor-rooted” is literal, not shorthand for canonicalizing before a
pathname walk. Pin `audio/` once, enumerate each opened directory descriptor,
and open every child relative to the verified parent. On kernels that support
it, use [`openat2`](https://www.man7.org/linux/man-pages/man2/openat2.2.html)
with `RESOLVE_NO_MAGICLINKS` plus the applicable
`RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`, and `RESOLVE_NO_XDEV` policy. The
portable Linux fallback is a bounded component-by-component `openat`/`fstat`
walk with `O_NOFOLLOW`; it never reopens an accumulated pathname. Directory
symlinks are reported and never followed.

File symlinks are enumeration records, not traversal edges. When enabled, a
separate bounded secure-open adapter resolves at most
`max_symlink_resolutions`, opens one verified regular final target, records
whether it is outside the root or crosses a mount, and never descends through
it. If the platform cannot uphold the no-magic-link, identity, or parent
stability contract, that symlink is reported as `unsupported_secure_open`;
there is no `std::fs::canonicalize` or path-based fallback. The opened handle,
not a later pathname re-open, is passed to metadata parsing and identity
capture.

Scanner bounds:

```txt
max files: 50000 by default
max encountered entries (files, directories, symlinks, and special files): 100000 by default
max depth: 16 by default
max LRC size: 256 KiB by default
max playlists: 2000 by default
max entries per playlist: 10000 by default
max path: 4 KiB each and 24 MiB total retained path capacity
max metadata field: 16 KiB
max total in-memory metadata: 48 MiB by default
max retained warnings: 8 MiB
max complete scan/search index: 112 MiB inclusive
```

Every encountered directory, file, symlink, special entry, parser/tag attempt,
symlink resolution, and mount-boundary decision consumes its own checked counter
before descent, resolution, open, or parser invocation. `max_files` counts only
accepted media entries; it cannot be bypassed with directories, ignored files,
or broken links. The default root-device policy is `cross_mounts = false`: the
scanner records and does not descend into a different-device directory;
file-symlink targets across a mount are still counted and may be read for
playback discovery only. A limit, mount boundary, non-regular target, or parser
refusal is one bounded structured warning, not a retry or a second scan.

The inclusive per-index budget counts every owned allocation and capacity for paths,
assets, entries, playlists, metadata, search fields, warnings, and lookup tables.
The narrower path/metadata/warning limits are sub-budgets, not additional memory
allowances. Preflight and incremental accounting stop at the first limit and
return a visibly partial diagnostic result; the previous complete index remains
active until a complete replacement scan succeeds. Before scanning, reserve the
replacement index and scan/parser scratch against the one process-wide budget
while retaining the old index; if reservation fails, do not start a scan.

Scan result should include warnings:

```txt
broken symlink
unsupported extension
decoder/tag read error
duplicate canonical path
ignored symlinked directory
unsupported secure file-symlink open
cross-mount directory skipped
too many files
too deep
```

The user should be able to open a diagnostics panel and see scan warnings. Silent skips are bad UX.

Every audio, tag, LRC, image, state, config, journal, and
backup parser consumes a verified regular-file descriptor or already bounded
bytes, never a pathname reopened after validation. Each parser has explicit
input-byte, line/item, nesting/record, field, decoded-output, and work-attempt
bounds before allocation or expensive decode. Audio probing and metadata reads
are finite: a scan parser attempt has a byte/read and time/work budget, does not
decode full media, and cannot follow a substituted final symlink. Unsupported,
non-regular, truncated, malformed, or exhausted input is a structured error.

`docs/parser-isolation.md` is the executable decision register for this
requirement. Before a parser enters a production phase, it records the exact
crate/API/version/features and proves either that the caller controls all input
bytes, reads, decoded output, nesting, work units, and cancellation points, or
that the parser runs in one of the bounded `max_parser_helpers` processes with
bounded IPC, CPU/address-space/file-descriptor limits, a hard timeout, kill, and
reap. A Tokio timeout around an uninterruptible in-process call is not accepted
as enforcement. Parser limit, helper crash, timeout, and cleanup cases are phase
acceptance criteria and fuzz targets arrive with the parser surface.

---

## Search Model

Suzumushi should have two related search surfaces:

```txt
/           audio search
Space+/    global palette
```

Audio search is for the local library only. It should search:

```txt
artist
album artist
album
title or episode name
playlist name
filename
relative folder path
```

The global palette should include the same music results plus commands:

```txt
rescan library
toggle shuffle
cycle repeat
open lyrics editor
open tag editor
open batch tag editor
toggle album art
toggle mini visualizer
show diagnostics
quit
```

Search rules:

- Use metadata first, filename/path fallback second
- Read artist/title/album tags during scan when cheap
- If tags are missing, derive searchable fallback text from path and filename
- Normalize case and Unicode for matching
- Strip terminal control characters before indexing
- Group results by Artists, Songs, Albums, Playlists, and Commands
- Prefer exact and prefix matches over fuzzy matches
- Prefer metadata matches over filename-only matches
- Bound visible results, for example 200
- Never block playback while filtering results
- Rebuild the search index only after a complete scan, rescan, tag edit, or playlist model change

Example audio search:

```txt
Search: lamp

Artists
  Lamp                         42 songs

Songs
  Lamp - Yume Utsutsu          Tokyo Utopia Tsushin
  Lamp - Hatachi no Koi        For Lovers

Albums
  Lamp - Tokyo Utopia Tsushin  12 songs

Playlists
  night-lamp                   18 songs
```

Result actions:

- Selecting an artist prompts replace vs append for all songs by that artist
- Selecting an album prompts replace vs append for all songs in that album
- Selecting a playlist prompts replace vs append for that playlist
- Selecting a song prompts replace vs append for that song
- Selecting a command in `Space+/` runs or confirms that command

The search index should stay in memory for v1. Do not introduce SQLite just for search. Revisit caching only if real libraries show that startup scanning is a problem after the folder-first model is stable.

---

## Audio Model

Queue/navigation actions exist only in the app/UI protocol:

```rust
pub enum QueueAction {
    Replace { items: Vec<QueuedTrack>, expected_queue_generation: u64 },
    Append { items: Vec<QueuedTrack>, expected_queue_generation: u64 },
    Remove { queue_item_id: QueueItemId, expected_queue_generation: u64 },
    Clear { expected_queue_generation: u64 },
    SetShuffle { enabled: bool, seed: Option<u64>, expected_queue_generation: u64 },
    SetRepeat { mode: RepeatMode, expected_queue_generation: u64 },
    Reorder { play_order: Vec<QueueItemId>, expected_queue_generation: u64 },
    Next { expected_queue_generation: u64, expected_playback_generation: u64 },
    Previous { expected_queue_generation: u64, expected_playback_generation: u64 },
}
```

The UI checks expected generations and item/byte preflight before committing any
action. No queue mutation command crosses into `audio/`.

Audio worker commands:

```rust
pub enum AudioCommand {
    LoadTrack {
        item: QueueItemSnapshot,
        expected_playback_generation: Option<u64>,
        new_playback_generation: u64,
    },
    Play { playback_generation: u64 },
    Pause { playback_generation: u64 },
    Stop { expected_playback_generation: u64, new_playback_generation: u64 },
    SeekRelative { delta: DurationDelta, expected_playback_generation: u64, new_playback_generation: u64 },
    SeekAbsolute { position: Duration, expected_playback_generation: u64, new_playback_generation: u64 },
    RecoverDevice { expected_playback_generation: u64, new_playback_generation: u64 },
    SetVolume { volume: f32, playback_generation: u64 },
    SetMuted { muted: bool, playback_generation: u64 },
    SetSpeed { speed: f32, playback_generation: u64 },
}
```

The UI/app loop is the sole allocator of monotonically increasing
`playback_generation` values. Transition commands carry both the expected old
generation and the reserved new generation; the first `LoadTrack` uses
`expected_playback_generation = None`. Ordinary commands carry the current
generation. The worker never invents a generation. Volume, mute, and speed
become acknowledged playback state only through `ControlAcknowledged`; the UI
may show a bounded pending indicator meanwhile. Device loss emits a reliable
`RecoveryRequired` event for the current item/generation, and the UI decides
whether to issue `RecoverDevice` with the next generation.

The audio worker also owns a monotonically increasing `audio_state_revision`.
It increments only for authoritative accepted state transitions: load, play,
pause, stop, seek, recovery, applied control changes, end, and error. Reliable
events carry that revision and are applied in reliable-channel order; telemetry
can never cause a reliable acknowledgement to be rejected. Position and
visualizer lanes each carry their own monotonically increasing sequence number
plus the state revision from which they were derived. The UI accepts telemetry
only when item and playback generation match, its state revision equals the
currently acknowledged revision, and its lane sequence is newer. Future-revision
telemetry that races ahead of its acknowledgement and stale telemetry are
dropped safely. `NowPlayingStateV1` persists the last acknowledged state
revision, not a telemetry arrival counter.

`PlayPause`, next, previous, repeat, shuffle, append, remove, clear, and replace
are app actions, not audio-worker ownership. The UI resolves `PlayPause` from its
acknowledged playback state and resolves navigation from its authoritative queue.

Audio worker events:

```rust
pub enum ReliableAudioEvent {
    Loaded { queue_item_id: QueueItemId, duration: Option<Duration>, queue_generation_at_load: u64, playback_generation: u64, audio_state_revision: u64 },
    StateChanged { queue_item_id: Option<QueueItemId>, state: PlaybackState, playback_generation: u64, audio_state_revision: u64 },
    SeekAcknowledged { queue_item_id: QueueItemId, source_position: Duration, playback_generation: u64, audio_state_revision: u64 },
    RecoveryRequired { queue_item_id: QueueItemId, playback_generation: u64, audio_state_revision: u64 },
    ControlAcknowledged { queue_item_id: Option<QueueItemId>, applied: AppliedAudioControls, playback_generation: u64, audio_state_revision: u64 },
    TrackEnded { queue_item_id: QueueItemId, playback_generation: u64, audio_state_revision: u64 },
    Error { queue_item_id: Option<QueueItemId>, error: AudioError, playback_generation: u64, audio_state_revision: u64 },
}

pub struct AppliedAudioControls {
    pub volume: f32,
    pub muted: bool,
    pub speed: f32,
}

pub struct PositionEvent {
    pub queue_item_id: QueueItemId,
    pub source_position: Duration,
    pub playback_generation: u64,
    pub audio_state_revision: u64,
    pub position_sequence: u64,
}
```

Rules:

- Clamp volume to configured bounds
- Mute preserves the configured volume and applies zero output gain; unmute restores that volume rather than guessing from a zero value
- Clamp speed to configured bounds
- Treat decoder failures as track errors, then advance only if the user configured skip-on-error later
- Never panic on a bad audio file
- Do not block the UI while opening or decoding a file
- If the audio output device disappears, show a recoverable error
- Playback position must remain accurate enough for lyrics, MPRIS, Waybar, tmux, Zellij, and notifications
- Pitch-preserving speed must be implemented without unbounded buffering
- Visualizer frames must be generated from samples already passing through Suzumushi playback, never by reopening files or capturing system audio
- Visualizer frames must be bounded and droppable if the UI lags
- Reliable events use their own bounded channel and cancellation-aware backpressure outside the real-time callback; position events are coalesced to the latest value and visualizer events are independently droppable
- `TrackEnded` never advances playback inside the worker. The UI accepts it only when `QueueItemId` and `playback_generation` match the active playback, resolves that ID against the current queue generation, mutates queue/history, and sends the next `LoadTrack` snapshot. Queue edits made after load do not invalidate the event.
- If `rodio` cannot support the required control path cleanly, move decoding/mixing behind a `symphonia`-based audio pipeline instead of weakening the v1 feature

Required v1 audio pipeline:

```txt
bounded decode -> format conversion -> pitch-preserving stretch -> volume/mute
               -> visualizer reduction -> bounded output buffer -> device
```

The audio playback work chooses the concrete decoder/output integration before
the worker architecture is committed. Rodio's native speed control changes pitch and therefore must not implement
v1 speed. The spike must prove supported codecs, 0.5x/1.0x/2.0x behavior,
seeking, device loss, PCM tap placement, bounded buffers, and shutdown using the
same ownership shape intended for production.

Playback clock invariants:

- `source_position` is the media timeline consumed by lyrics, progress, MPRIS, and status outputs. At 2.0x it advances about two media seconds per wall second; at 0.5x it advances about half a media second.
- The audio worker derives source position from frames actually committed to the bounded output path and compensates for known output/stretch latency; the UI never invents position from wall time.
- Pause freezes source position. Seek establishes a new playback generation, clears decoded/stretcher/output buffers, and emits the acknowledged source position before normal progress resumes.
- End-of-track fires once after the final audible buffered sample, not merely when decoding finishes.
- Position is monotonic within a generation, clamped to known duration, and never belongs to a stale queue item.
- A queue mutation changes `queue_generation`; loading a track, seeking, stopping/restarting, or recovering the device changes the UI-owned `playback_generation`. Queue actions carry expected queue generation, while active playback commands/events/projections are matched by item plus playback generation. Queue generation at load may be recorded for diagnostics but is never an active-event rejection condition.

V1 speed behavior:

```txt
0.5x, 0.75x, 1.0x, 1.25x, 1.5x, 1.75x, 2.0x
```

Speed changes must preserve pitch in v1.

Time-stretch rules:

- `1.0x` should bypass the stretcher
- `0.5x` and `2.0x` are required bounds, not stretch goals
- Buffer sizes must be explicit and bounded
- Seeking should reset stretch state to avoid audio artifacts
- If stretching fails for a track, report an audio error instead of silently falling back to pitch-changing speed
- Pitch preservation has no user-facing config bypass in v1. The deterministic
  diagnostic harness may compare a stretcher bypass, but normal playback never
  falls back to pitch-changing speed.

---

## Visualizer Model

The v1 visualizer should be a small audio-reactive level display, not a full spectrum analyzer.

Suggested frame model:

```rust
pub struct VisualizerFrame {
    pub queue_item_id: QueueItemId,
    pub queue_generation_at_load: u64, // diagnostics only
    pub playback_generation: u64,
    pub audio_state_revision: u64,
    pub visualizer_sequence: u64,
    pub levels: [u8; MAX_VISUALIZER_BARS], // preallocated; each value is clamped to 0..=8
    pub level_count: u8,
    pub peak: u8,
}
```

Display examples:

```txt
(( ▁▂▄▆█▅▃▂ ))
▁▂▄▇▆▃▂▁  ▁▃▅█▆▄▂▁
```

V1 rules:

- Use short RMS/peak windows from decoded PCM to produce level bars
- Keep the default subtle: 16 bars, one terminal row, soft falloff
- Cap bars to a small fixed maximum, for example 32
- Cap frame updates to about 8-12 Hz
- Derive frames from the playback path after volume/mute so the visualizer reflects audible output
- Never decode the same audio file a second time only for visualization
- Never capture microphone or system audio
- Do not fake random motion when stopped, paused, or silent
- Render idle states with Suzu motifs such as `((•))` or `. z z`
- Drop stale frames when the track changes, seek generation changes, or the UI channel is full
- The real-time producer fills the fixed-size frame or a preallocated pool; it never allocates a `Vec` or acquires a lock.
- Do all sample reduction off the render path; the TUI only draws the latest `VisualizerFrame`
- Defer FFT frequency-spectrum visualizers until after v1

The visualizer should feel like the song is gently breathing in the terminal. It should not compete with lyrics or album art.

---

## Mutation And Backup Model

Lyric and tag editors share one `backups/` foundation rather than implementing
different safety semantics. Before lyric editing is enabled, Suzumushi provides
generic `backups list`, `verify`, `restore`, and previewed `prune` operations,
checked byte/entry budgets, content verification, and recovery on startup. Tag
editing reuses this implementation and adds only its format-specific payload and
post-write validation.

Every mutation has a durable journal/manifest containing operation ID, kind,
phase, source and canonical destination, verified file identity, preflight
size/hash or an explicit verified-absent preimage marker, backup reference/hash
when one exists, temporary path, intended post-write hash where known,
timestamps, tool version, and recovery disposition. Journal transitions
are flushed and synced before the corresponding filesystem transition. Startup
under the root writer lock deterministically evaluates every incomplete operation
and records `Completed`, `RolledBack`, `RecoveryRequired`, or `Unrecoverable`; it
never guesses from temporary filenames alone or promises rollback without a
sufficient preimage.

Journals are versioned, length- and byte-bounded records with a checksum and
atomic descriptor-relative replacement. Recovery treats every journal as
untrusted input: malformed JSON, unknown schema/phase, duplicate or impossible
transition, checksum mismatch, truncation, missing referenced file, or failed
descriptor verification becomes a persisted `RecoveryRequired` or
`Unrecoverable` diagnostic with the raw bounded record retained for audit. It
does not use assertions, panic, infer a successful write, or delete evidence.
New mutation work for that target remains blocked until a verified recovery
decision completes.

Journal and manifest storage is bounded in aggregate, not merely per record.
`journal_record_max_bytes`, `journal_max_entries`,
`journal_max_total_bytes`, tag/lyric backup entry limits, and
`startup_recovery_max_records` are checked before creation or startup parsing.
At a limit, Suzumushi preserves all existing records, streams one bounded
diagnostic summary, blocks new mutations, and directs the user to verified
list/restore/prune recovery; it never deletes the oldest evidence automatically.
Startup work cannot be amplified by many tiny malformed records.

Mutation lifecycles:

```txt
new lyric:
preview -> verify destination absent -> durable prepared journal with absent marker
        -> commit -> write/flush/sync/rename -> parent sync -> reopen/verify -> complete

existing lyric / atomic replacement:
preview -> identity/budget preflight -> verified backup -> durable prepared journal
        -> commit -> write/flush/sync/rename -> parent sync -> reopen/verify
        -> complete or rollback from verified backup

tag mutation with full-file backup:
preview -> identity/budget preflight -> verified full backup -> durable prepared journal
        -> commit using verified open FileLike -> reopen/verify
        -> complete or restore full file from verified backup

```

Cancellation is accepted only before commit begins. Once backup/write commit
begins, quit and normal worker cancellation stop accepting new work but await
durable completion or a journaled recoverable state. The root lock remains held
for that entire interval, independent of `shutdown_timeout_ms`; the global TUI
lock also remains held while the process is still serving the mutation. A
mutation task is never detached. If durable completion cannot be established,
shutdown reports the blocking operation and continues waiting or terminates only
through an explicit hard-abort path whose next startup must recover the journal.
A watchdog continues reporting the operation ID, phase, elapsed time, and recovery file while waiting. The documented hard-abort procedure never deletes locks, journals, or temporary files manually; it terminates the process and requires the next startup under the root lock to recover before new mutation work begins.

Backup list/verify/restore/prune and all writes pin and verify the app-owned
`state/`, `backups/`, `logs/`, and cache directory identities beneath the
canonical root, reject substituted/symlinked parent components, use
descriptor-relative no-follow checks, revalidate identity at use time, enforce
their configured budgets with checked arithmetic, and produce an auditable
result. Prune is always previewed and confirmed; it
cannot remove a backup referenced by an incomplete mutation or retained recovery
record.

Restore is a separate destructive journaled mutation. It previews the target and
backup hashes plus exact path, requires confirmation, refuses any target loaded
by the audio worker or resolved outside the canonical root, revalidates target identity and symlink state,
and preserves the current target in a new verified full-file backup before
overwrite. If that preservation would exceed the budget, restore fails closed;
it never silently spends or deletes the backup being restored. External targets
are unconditionally read/play-only and no confirmation overrides that boundary.
Restore uses the same verified same-parent descriptor-relative atomic replacement
contract as other mutations; if the required operation is unavailable it returns
`unsupported`, never a weaker path-based or non-atomic fallback. It reopens and
verifies the result and retains both operation records for audit and rollback.

---

## Lyrics Model

LRC line model:

```rust
pub struct LrcDocument {
    pub original_bytes: Vec<u8>,
    pub encoding: LrcEncoding,
    pub newline_style: NewlineStyle,
    pub had_final_newline: bool,
    pub nodes: Vec<LrcNode>,
    pub timed_lines: Vec<LyricLine>,
}

pub struct LyricLine {
    pub timestamp: Duration,
    pub text: String,
}

pub enum LrcNode {
    TimedLine { source_range: Range<usize> },
    Metadata { source_range: Range<usize> },
    Unsupported { source_range: Range<usize> },
    Blank { source_range: Range<usize> },
}
```

Parser rules:

- Accept `[mm:ss.xx]` timestamps
- Accept multiple timestamps for one lyric line
- Sort by timestamp after parsing
- Preserve duplicate timestamps in input order
- Require valid UTF-8 for semantic parsing/editing in v1. Invalid UTF-8 is reported as unsupported and may be displayed safely as a diagnostic, but is never lossy-decoded or overwritten by the editor.
- Preserve the original bounded bytes, per-line source ranges, LF versus CRLF style, trailing whitespace, unsupported lines, metadata, blank lines, and presence/absence of the final newline
- Build normalized semantic timed nodes that reference source ranges; normalization never replaces the lossless source representation
- Exclude unsupported metadata from timing semantics but retain its exact source bytes so unchanged content round-trips byte-for-byte
- Reject unsafe control characters for semantic editing/display without rewriting unchanged source bytes
- Enforce file size and line count limits

Display behavior:

```txt
previous lyric line: dim
current lyric line: bright
next lyric lines: muted
```

Lyrics should update from playback position, not from an independent wall-clock timer. Pause, seek, speed, next, and previous must all resolve to the correct active line.

Lyric edit behavior:

```txt
focused lyrics panel -> edit .lrc -> validate timestamps -> preview parse result -> existing: verified backup / new: verified-absent marker -> atomic save -> reload lyrics
```

Lyric edit rules:

- Create a sidecar `.lrc` when no lyric file exists and the target path is writable
- Edit shared `audio/lyrics/` files when that is the active lyric source
- Show whether the lyric file is sidecar, canonical-target sidecar, or shared
- Back up the previous lyric file under `suzumushi/backups/lyric-edits/`
- Reject invalid timestamps on save
- Preserve unknown bounded LRC metadata and unsupported lines byte-for-byte unless the user edits that line explicitly
- Preserve original newline style, trailing whitespace, blank lines, and final-newline state for every unchanged line; serialize only explicitly edited semantic nodes
- Do not block playback while editing lyrics
- Revalidate sidecar/shared/symlink-target identity immediately before save. For a symlinked entry, the user chooses playlist-sidecar or canonical-target-sidecar explicitly; the app never changes this destination implicitly.
- The selected lyric destination must resolve beneath the canonical root; an external canonical-target sidecar is refused in v1 even after selection.
- Create the temporary file through the pinned destination-parent descriptor, flush and sync it, use descriptor-relative same-parent `renameat2` when available, then sync that parent. If required atomic/no-replace semantics are unavailable, fail explicitly; never silently use a non-atomic fallback.
- Preserve existing permissions on replacement, use user-only permissions for a new file, and operate only on verified regular descriptors; never replace a symlink object accidentally when the intended target was its referent
- Existing-file saves require checked preflight against lyric backup byte and entry budgets and a verified backup journal entry before mutation commit

---

## Tag Model

Tag read behavior:

- Read common tags during scan when cheap
- Fill missing title from filename
- Fill missing artist/album as `Unknown Artist` / `Unknown Album` only in UI, not in file tags
- Keep raw missing values as `None` internally

Tag edit behavior:

```txt
selected track -> tag editor -> edit fields -> preview diff -> confirm save -> backup -> write -> rescan track
```

Batch tag edit behavior:

```txt
mark tracks -> batch tag editor -> choose fields -> preview every affected file -> backup each file -> write -> rescan changed tracks
```

Preview should show:

```txt
File: suzumushi/audio/library/music/artist/album/01 song.flac
Target: same file

Title:  Old Title  -> New Title
Artist: Old Artist -> New Artist
Album:  unchanged
```

For symlinked playlist files:

```txt
Playlist entry: suzumushi/audio/playlists/focus/01 song.flac
Real target:    /some/path/suzumushi/audio/library/music/artist/album/01 song.flac

Saving edits will modify the real target.
```

Backup rules:

- Full-file backup mode backs up and verifies the full original file before every write; identical content may reuse a verified content-addressed backup
- Use a content/path hash in backup metadata so repeated edits can be traced
- Store a bounded versioned manifest for each actual mutation variant: verified-absent lyric creation, existing-lyric replacement, full-file tag backup, and destructive restore preservation
- Refuse to write if backup budget would be exceeded
- Refuse to write if the source file changes between preview and save
- Revalidate the symlink itself and the opened target's file identity immediately before backup and immediately before write; metadata timestamps alone are insufficient
- Refuse any tag mutation whose final resolved target is outside the canonical Suzumushi root; external symlink targets are read/play-only in v1 and no confirmation can override this
- Refuse tag mutation of a file with link count other than one in v1; a hard-linked target could mutate a name outside the root or a user-unreviewed alias
- Open the verified regular target through a no-follow descriptor and perform tag work only through a selected library/adapter contract that can write a same-parent descriptor-relative temporary replacement and `renameat2`; if that format/library cannot support the verified atomic contract, fail as unsupported rather than reopening or overwriting by path
- After writing, reopen and parse the file through a verified regular descriptor; verify requested fields and byte-for-byte preservation of every unedited metadata item, tag block, picture, attachment, chapter, and format-supported audio/container property. On incomplete preservation evidence, restore from the verified backup and report failure.
- Preserve documented permissions and report unsupported ownership/xattr behavior; do not claim a fully atomic tag rewrite when the format/library cannot provide one

Do not edit a file while it remains loaded/open in the audio worker, including
paused or stopped-but-not-unloaded state. Require a matching unload
acknowledgment first. This avoids output-device and decoder races and keeps the
write path simple to reason about.

Batch edit rules:

- Apply only fields explicitly selected in the batch editor
- Never blank a field by accident
- Show the exact file count before saving
- Deduplicate marked contextual entries by verified physical file identity before preview/count; reject conflicting requested changes and write each target at most once
- Refuse the batch if it contains any file currently loaded/open by the audio worker
- Stop on first write failure unless the user explicitly chooses a later continue-on-error mode
- Keep the default max batch size small enough to review, for example 200 files
- Produce a batch summary after completion

---

## Artwork Model

Artwork sources:

```txt
embedded image tag
cover.jpg / cover.png beside audio file
folder.jpg / folder.png beside audio file
canonical target directory for symlinked playlist entries
```

Artwork cache:

```txt
suzumushi/state/artwork/<hash>.png
```

Rules:

- Decode artwork off the UI render path
- Normalize cached artwork to a bounded PNG size
- Keep a versioned cache manifest with content key, transformed byte length, last-used generation, and bounded pin leases; rebuild it from verified regular cache entries if it is corrupt
- Bound source image bytes and decoded dimensions
- Treat image files and embedded images as untrusted input
- Enforce compressed/source bytes, width, height, pixel count, and decoded RGBA bytes before allocating the full image; `preferred_size_px` is not a safety limit
- Cache writes use a no-follow, same-directory atomic replacement and a verified manifest; cleanup never follows cache symlinks
- Before admitting a cache entry, reserve its exact transformed bytes. Evict unpinned entries by ascending `last_used_generation`, breaking ties by lexical content hash, until the entry fits. Cache hits advance one checked monotonic use generation; wrap is a typed cache-reset event, not silent misordering.
- The artwork currently published by MPRIS is pinned until replacement metadata is acknowledged. Notification artwork receives a boot-clock lease through `notification_timeout_ms + 5_000 ms`; at most four notification leases exist, and additional notifications use no image rather than exceeding the cache budget. Pins and leases count inside `max_cache_bytes`.
- If current/pending pins leave insufficient budget for new artwork, first publish the text fallback and remove the old external reference, then unpin and retry admission. Never delete a path still advertised by MPRIS or a live notification lease, and never exceed the cache budget for a transition.
- Startup removes verified orphan temporary files and expires notification leases from an earlier boot before normal eviction; active MPRIS state is repinned from the validated now-playing projection
- If terminal image protocol detection fails, render a text fallback
- Use cached artwork path for MPRIS and notifications
- Do not edit embedded artwork in v1

---

## MPRIS Model

MPRIS metadata should include what desktop widgets expect:

```txt
mpris:trackid
mpris:length
mpris:artUrl
xesam:title
xesam:artist
xesam:album
xesam:albumArtist
xesam:trackNumber
xesam:genre
```

Playback status:

```txt
Playing
Paused
Stopped
```

Supported command mapping:

```txt
MPRIS Play        -> AppAction::Play
MPRIS Pause       -> AppAction::Pause
MPRIS PlayPause   -> AppAction::PlayPause
MPRIS Stop        -> AppAction::Stop
MPRIS Next        -> AppAction::Next
MPRIS Previous    -> AppAction::Previous
MPRIS Seek        -> AppAction::SeekRelative
MPRIS SetPosition -> AppAction::SeekAbsolute, if track ID and playback generation match
```

Invariants:

- MPRIS commands must go through the same command path as keyboard input
- MPRIS must not mutate UI state directly
- MPRIS metadata must update on track change and tag edit
- MPRIS album art must use a local file URI from the bounded artwork cache
- Build file URIs with a URL/file-URI API so spaces, Unicode, and reserved bytes are encoded correctly
- If the active track changes, stale `SetPosition` calls must be ignored
- The app loop resolves queue navigation with expected queue generation and sends audio commands with item plus UI-owned playback generation; MPRIS never owns queue or generation state
- If D-Bus fails, playback must continue
- Advertise only capabilities that are implemented. Unsupported `OpenUri`, `Raise`, `Quit`, loop, shuffle, or volume operations are rejected/disabled explicitly rather than mapped to surprising behavior.

---

## Waybar Model

Single shared state file:

```txt
suzumushi/state/now-playing.json
```

Suggested bounded schema shared by all status renderers:

```rust
pub struct NowPlayingStateV1 {
    pub schema_version: u16,
    pub writer_pid: u32,
    pub writer_start_ticks: u64,
    pub writer_instance_id: String,
    pub writer_boot_id: String,
    pub writer_state: StatusWriterState,
    pub playback_generation: u64,
    pub audio_state_revision: u64,
    pub queue_generation: u64,
    pub queue_item_id: Option<QueueItemId>,
    pub updated_boottime_ms: u64,
    pub updated_unix_ms: i64,
    pub playback: PlaybackState,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub source_position_ms: u64,
    pub position_sample_boottime_ms: u64,
    pub duration_ms: Option<u64>,
    pub volume_percent: u8,
    pub muted: bool,
    pub speed_milli: u16,
    pub lyrics_available: bool,
    pub artwork_cache_path: Option<PathBuf>,
}

pub enum StatusWriterState {
    Active,
    GracefulStopped,
}
```

All strings and serialized bytes obey status limits. Unknown schema versions are
not guessed. `writer_instance_id` is a bounded random non-secret identifier also
recorded in the per-user active-TUI lock; together with Linux process start
ticks and the bounded current boot ID it prevents PID reuse or a stale
pre-reboot file from making a crashed writer look live. `updated_unix_ms` is
diagnostic only. `updated_boottime_ms` and `position_sample_boottime_ms` use
suspend-aware
[`CLOCK_BOOTTIME`](https://www.man7.org/linux/man-pages/man2/clock_gettime.2.html)
and are never compared across different boot
IDs.

All three status renderers share one liveness validator. `Active` state requires
a current boot-ID match, non-future boot-clock samples, a live
PID/process-start/instance match, and a matching held per-user active-TUI lock.
There is no wall-clock freshness decision and no periodic disk heartbeat.
While `playback == Playing`, a renderer extrapolates source position from the
last acknowledged `source_position_ms`, checked elapsed `CLOCK_BOOTTIME`, and
`speed_milli`, then clamps to duration; paused and stopped positions never
advance. Backward wall-clock jumps do not affect liveness or position.
`GracefulStopped` is accepted as a calm stopped result only when playback is
stopped and no per-user active-TUI lock exists. Any boot/lock/process identity
mismatch, PID reuse, future boot-clock sample, arithmetic failure, or
uncheckable identity returns the renderer's bounded visible error fallback
rather than stale now-playing data. A genuinely missing state file also returns
the calm stopped fallback.

Write behavior:

- Update on track change, load error, or active metadata/tag change
- Mark the latest state dirty after every committed queue mutation because the persisted schema includes `queue_generation`; repeated mutations coalesce rather than enqueueing one disk transaction each
- Update on play/pause/stop, volume, mute, speed, seek acknowledgment, device recovery, lyrics-source availability, and artwork-cache completion
- One owned state-writer task consumes a capacity-one latest-value/watch slot and performs event-driven writes at most once per `status_write_coalesce_ms`. Initial `Active` and final `GracefulStopped` are the only out-of-cadence lifecycle writes. Track changes, errors, queue mutations, and acknowledged playback-state/position changes replace the pending latest value and flush at the next permit; ordinary playing progress causes no write because renderers extrapolate it.
- Graceful shutdown closes admission, awaits any in-flight atomic write, writes the newest state exactly once as `GracefulStopped`, then releases the per-user active-TUI lock. A writer error remains visible and bounded; it never creates an unbounded retry queue.
- Write a final `GracefulStopped` projection on graceful shutdown before releasing the writer lock; crash/forced-termination state remains `Active` and is rejected through boot, process, instance, and lock identity
- Write through the pinned `state/` parent descriptor and same-parent `renameat2`; report unsupported/non-atomic failure rather than falling back
- Serialize the versioned `NowPlayingState` with `serde_json`, never hand-built JSON strings
- Create the temporary file relative to the verified `state/` parent, refuse a symlink/non-regular destination, finish serialization before rename, and keep user-only permissions where supported. This file is a reconstructible projection: do not `fsync` the temporary file or parent directory. A crash may lose the latest projection and status commands then use their documented safe fallback; durable mutation paths use the separate journal/sync contract.
- Enforce the state byte limit before writing and before any status command deserializes it
- Include schema version, PID, process start ticks, writer-instance ID, boot ID, queue generation, playback generation, queue item ID, update boot-clock sample, diagnostic Unix time, and position boot-clock sample so stale/corrupt/PID-reused/pre-reboot state is detectable

Example rendered Waybar outputs derived from the state file (not the persisted
`NowPlayingStateV1` schema itself):

```json
{"text":"Nothing playing","tooltip":"Suzumushi is stopped","class":["stopped"],"percentage":0}
```

```json
{"text":"Lamp - Yume Utsutsu","tooltip":"Tokyo Utopia Tsushin\n01:42 / 04:05\nSuzumushi","class":["playing"],"percentage":41}
```

Recommended classes:

```txt
playing
paused
stopped
muted
lyrics
no-lyrics
error
```

Waybar command behavior:

- If state exists, apply the shared validator: `Active` data requires bounded byte/schema/version checks, current boot ID, non-future `CLOCK_BOOTTIME` samples, and PID/process-start/writer-instance match against the held per-user active-TUI lock; valid `GracefulStopped` data requires stopped playback and no active-TUI lock
- If state file is missing, print a stopped JSON state and exit with success
- If JSON is corrupt, print an error class and exit with success; corruption is surfaced visibly rather than disguised as a stopped state, because silent skips are bad UX
- Do not block waiting for the TUI
- Treat artist/title/album as plain text. The documented Waybar config sets `escape = true`; do not allow metadata to become Pango markup

---

## Tmux Model

State file:

```txt
suzumushi/state/now-playing.json
```

Example output:

```txt
♪ Lamp - Yume Utsutsu 01:42/04:05
```

Tmux command behavior:

- Apply the same bounded schema, boot-ID/boot-clock, PID/process-start, and writer-instance/active-lock liveness validation as Waybar before rendering live data
- Print one sanitized line and exit
- Keep output short enough for status bars
- Use no terminal escape sequences
- Bound input and output bytes and neutralize terminal controls plus any tmux format metacharacters proven active in the documented embedding path
- Return a calm stopped state when nothing is playing
- If state is missing, return `Suzumushi stopped` with success; if state is corrupt, return the short `suzumushi: state error` string with success so corruption stays visible in the status bar, matching the Waybar error class
- Do not start audio, scan music, or open the TUI

---

## Zellij Model

State file:

```txt
suzumushi/state/now-playing.json
```

Command:

```txt
suzumushi zellij --root /absolute/path/to/suzumushi
```

Example output:

```txt
♪ Lamp - Yume Utsutsu 01:42/04:05
```

Suggested `zjstatus` config:

```kdl
command_suzumushi_command "suzumushi zellij --root /home/user/path/to/suzumushi"
command_suzumushi_format "#[fg=green] {stdout} "
command_suzumushi_interval "1"
command_suzumushi_rendermode "static"
```

Then include it in the status format:

```kdl
format_right "{command_suzumushi} {datetime}"
```

Zellij command behavior:

- Read bounded, versioned `suzumushi/state/now-playing.json` and apply the same boot-ID/boot-clock, PID/process-start, and writer-instance/active-lock liveness validation before rendering live data
- Print one sanitized line and exit
- Keep output short enough for status bars
- Use no terminal escape sequences by default
- Bound input and output bytes and neutralize terminal controls plus any `zjstatus` formatting metacharacters proven active in the documented embedding path
- Return a calm stopped state when nothing is playing
- If state is missing, return `Suzumushi stopped` with success; if state is corrupt, return the short `suzumushi: state error` string with success so corruption stays visible in the status bar, matching the Waybar error class
- Do not start audio, scan music, open the TUI, or talk to the Zellij session directly
- Document `zjstatus` as the recommended v1 integration path instead of shipping a custom Zellij plugin

---

## UI Direction

Suzumushi should feel like a quiet cassette deck, a small night radio, or a peaceful room.

Design words:

```txt
calm
warm
minimal
soft contrast
late night
focused
```

Avoid:

```txt
generic file manager look
too many borders
dashboard noise
giant equalizer gimmicks
fake random visualizer animations
bright cyberpunk by default
```

Suggested default layout:

```txt
+----------------------+----------------------------+----------------------+
| Library              | Now Playing                | Queue                |
|                      |                            |                      |
| Library              | Lamp - Yume Utsutsu        | 01 Yume Utsutsu      |
| Playlists            | Tokyo Utopia Tsushin       | 02 Rain Song         |
|   focus              | (( ▁▂▄▆█▅▃▂ ))             | 03 Quiet Morning     |
|   night-drive        | [=======>------] 01:42     |                      |
|   sleep              | vol 70%  speed 1.0x  V:on  |                      |
+----------------------+----------------------------+----------------------+
| Art                  | Lyrics                                            |
| cover image or       |        the current lyric line glows softly        |
| quiet text fallback  |            the next lyric line waits              |
+----------------------------------------------------------------------------+
| NORMAL  Space play/pause  n next  p prev  Space+/ palette  A art  ? help     |
+----------------------------------------------------------------------------+
```

Small terminal fallback:

```txt
Suzumushi needs at least 80x24.
Resize the terminal or press q to quit.
```

Color and theme contract for v1:

```txt
terminal   inherit the terminal emulator's foreground, background, and ANSI palette
mono       no-color fallback using text attributes and symbols only
```

`terminal` is the default and is the primary visual theme. Suzumushi must fit the
user's existing terminal theme, including Gruvbox and other custom palettes,
without trying to identify or reproduce that theme itself.

Terminal-native color rules:

- Use the terminal's default foreground and background for normal surfaces; do not paint a global background.
- Use only named ANSI color slots when color is needed, so the terminal emulator supplies their actual RGB values.
- Do not hard-code RGB colors, assume that black means dark or white means light, or select colors from a bundled dark/light palette.
- Use attributes such as bold, dim, underline, and reverse plus symbols and spacing for essential hierarchy; color is supplementary and never the only state indicator.
- Keep contrast readable on both light and dark terminal backgrounds. Prefer default foreground/background or reverse video for selections when an ANSI accent cannot guarantee contrast.
- Respect `NO_COLOR`, an explicit `theme = "mono"`, and terminals without color support.
- Album-art pixels are media content and are exempt from the UI palette rule, but surrounding panels and fallbacks remain terminal-native.

No-color support arrives with the first styled UI, not as late polish. Local audio
and every control must remain usable on a plain terminal.

The mini visualizer belongs in the Now Playing panel. It should use soft bars, Suzu wing/ring motifs, or a mono-safe fallback. It should disappear or become a tiny idle motif in small terminals rather than pushing out lyrics, queue, or status controls.

---

## Phases

The phases below move Suzumushi from a small Rust setup to a polished Linux local audio player. The order matters: establish the package, build the root scanner and queue, choose the audio path when playback begins, then add consumers and file-writing features only when their foundations exist.

Each phase should end with `cargo run -- --root ./suzumushi` or a small CLI command showing something real. Avoid a big-bang build where playback, MPRIS, notifications, artwork, lyrics, and tag writes all land at once.

### Progress Board

**How to use the boxes**

- Every phase below carries a `**Status:**` line. Move it through `not started` -> `in progress` -> `blocked` -> `done`.
- Every build step and every `Done when` condition is a checkbox. Tick `[x]` when it is actually true, not when it is planned.
- Build steps are the work. `Done when` is the gate. A phase is `done` only when every `Done when` box is ticked.
- `Do not build yet` lists stay unticked on purpose - they are prohibitions, not tasks.
- This board is the single at-a-glance view. Keep it in sync with the per-phase `**Status:**` lines.

**Phases**

- [x] **Phase 1** - Project Setup
- [x] **Phase 2** - Suzumushi Root And Scanner - *milestone: Local root prototype* - *tag 0.1.0*
- [ ] **Phase 3** - Minimal TUI Shell - *milestone: Terminal prototype* - *tag 0.2.0*
- [ ] **Phase 4** - Library, Search, Playlist Browser, And Queue
- [ ] **Phase 5** - Audio Playback Worker
- [ ] **Phase 6** - Playback Controls, Pitch-Preserving Speed, Mini Visualizer, And State Files - *milestone: Playable core* - *tag 0.3.0*
- [ ] **Phase 7** - Synced `.lrc` Lyrics And Local Lyric Editing - *milestone: Lyrics editor MVP* - *tag 0.4.0*
- [ ] **Phase 8** - Album Art Pipeline And Terminal Display - *milestone: Artwork MVP* - *tag 0.5.0*
- [ ] **Phase 9** - Waybar, Tmux, And Zellij Status Outputs - *milestone: Status outputs MVP* - *tag 0.6.0*
- [ ] **Phase 10** - MPRIS, Media Keys, Notifications, And MPRIS Album Art - *milestone: Desktop MVP* - *tag 0.7.0*
- [ ] **Phase 11** - Safe Single-Track And Batch Tag Editing - *milestone: Safe metadata MVP* - *tag 0.8.0*
- [ ] **Phase 12** - Search Palette Polish, Diagnostics, And V1 Fit-And-Finish - *milestone: V1 fit-and-finish* - *tag 0.9.0*
- [ ] **Phase 13** - Linux Release Candidate - *milestone: Release candidate* - *tags 1.0.0-rc.N then 1.0.0*

Milestones:

| Milestone | Finished after | Meaning |
|---|---:|---|
| Local root prototype | Phase 2 | `suzumushi init` creates the folder model and scan diagnostics work |
| Terminal prototype | Phase 3 | TUI opens, resizes, handles keys, and exits cleanly |
| Playable core | Phase 6 | Folder playlists and search results play with queue, volume, seek, shuffle, repeat, pitch-preserving speed, and a subtle mini visualizer |
| Lyrics editor MVP | Phase 7 | `.lrc` lyrics sync, shared lookup works, and local lyric edits save safely |
| Artwork MVP | Phase 8 | Album art is extracted, cached, and displayed in supported terminals |
| Status outputs MVP | Phase 9 | Waybar, tmux, and Zellij commands expose now-playing state |
| Desktop MVP | Phase 10 | MPRIS/media keys, MPRIS album art, and track-change notifications work |
| Safe metadata MVP | Phase 11 | Single-track and batch tag edits work with backups and confirmations |
| V1 fit-and-finish | Phase 12 | Global palette, diagnostics, terminal-native theme, and empty/error states are release-grade |
| Release candidate | Phase 13 | Docs, tests, packaging, and Linux QA are clean |

Versioning and changelog:

Suzumushi follows SemVer. `CHANGELOG.md` (Keep a Changelog format) is created during project setup with an `Unreleased` section; each phase summarizes its user-visible changes under `Unreleased` as it completes. Git tags always use a `v` prefix (`v0.1.0`, `v1.0.0-rc.1`, `v1.0.0`), while `Cargo.toml` and changelog versions remain plain SemVer. For each milestone, a reviewed release commit moves that content into the dated version section and bumps `Cargo.toml`. The maintainer runs `mise run ci` and focused packaged-artifact QA before pushing the tag; the single release workflow then builds the tagged source and publishes archives and checksums.

| Version | Tagged after | Milestone |
|---|---:|---|
| 0.1.0 | Phase 2 | Local root prototype |
| 0.2.0 | Phase 3 | Terminal prototype |
| 0.3.0 | Phase 6 | Playable core |
| 0.4.0 | Phase 7 | Lyrics editor MVP |
| 0.5.0 | Phase 8 | Artwork MVP |
| 0.6.0 | Phase 9 | Status outputs MVP |
| 0.7.0 | Phase 10 | Desktop MVP |
| 0.8.0 | Phase 11 | Safe metadata MVP |
| 0.9.0 | Phase 12 | V1 fit-and-finish |
| 1.0.0-rc.N | Phase 13 | Release candidate |
| 1.0.0 | Phase 13 gates pass | First advertised Linux release |

Phases without a tag row ship their work in the next tagged release. Pre-1.0 tags are development snapshots: CLI flags, config schema, keybindings, and state-file layout may change between minor versions, with every break recorded in the changelog (the versioned `now-playing.json` contract still bumps its own schema version on change). `v1.0.0` is created only when every release-candidate gate passes; it is the v1 scope this document describes. After package version `1.0.0`, a breaking change to CLI flags, config schema, keybindings, the folder/root model, state-file schemas, or backup/journal formats requires a major bump; new features bump minor; fixes bump patch.

Recommended order:

```txt
minimal Rust project setup
-> suzumushi root and scanner
-> terminal shell
-> library, search, and queue
-> choose one audio path and implement playback
-> pitch-preserving controls, mini visualizer, and state
-> synced lyrics and lyric editing
-> artwork extraction and terminal art
-> Waybar, tmux, and Zellij output
-> MPRIS, media keys, notifications
-> single and batch tag editing
-> search, diagnostics, polish
-> packaging and release
```

---

### Phase 1 - Project Setup

**Status:** `done`

**Goal:** Finish a small, reproducible Rust package before feature work begins.

**Expected result:** `cargo run -- --help` works and the same focused checks run locally and in CI.

#### Build steps

1. [x] Finish the root `suzumushi` package metadata with edition `2024`, `rust-version = "1.95.0"`, Apache-2.0, `default-run = "suzumushi"`, strict lints, and committed `Cargo.lock`.
2. [x] Keep one canonical `suzumushi` binary and one library target. `main.rs` contains only startup/CLI wiring; `lib.rs` owns implementation exposed to tests.
3. [x] Forbid application `unsafe` and add `SPDX-License-Identifier: Apache-2.0` headers to Rust sources.
4. [x] Add only the minimal dependency needed for CLI parsing, pinned exactly with minimal features. Do not add audio, TUI, parser, fuzz, or desktop dependencies yet.
5. [x] Create `mise.toml` with Rust `1.97.1` and focused `fmt`, `clippy`, `test`, `msrv`, and `ci` tasks. The `msrv` task alone selects Rust `1.95.0`; all Cargo commands use `--locked`.
6. [x] Keep `.gitignore`, `LICENSE`, and `CHANGELOG.md` with an `Unreleased` section under version control.
7. [x] Add one `.github/workflows/ci.yml` that installs pinned `mise` and runs `mise run ci`.
8. [x] Add the smallest CLI test that proves `--help` succeeds and names the canonical command. Do not scaffold future commands or modules merely to test placeholders.

#### Done when

- [x] `mise run ci` passes on Rust `1.97.1` and the Rust `1.95.0` MSRV task passes.
- [x] `cargo run --locked -- --help` succeeds.
- [x] Cargo exposes one binary and one library target with no application features.
- [x] No dependency or module exists without behavior required by this phase.

#### Do not build yet

- Scanner, TUI, queue, audio, desktop integrations, editors, fuzzing, or release automation.
- Security or parser documentation before a real trust boundary is introduced.

---

### Phase 2 - Suzumushi Root And Scanner

**Status:** `done`

**Goal:** Create and scan the app-owned `suzumushi/audio/` folder safely.

**Why this comes next:** The filesystem model is the product. Audio playback should be built against real scanned tracks, not hardcoded paths.

**Expected result:** `suzumushi init ./suzumushi` creates the directory structure, and `suzumushi diagnose --root ./suzumushi` reports discovered tracks, lyrics, artwork candidates, and warnings.

#### Build steps

1. [x] Create `cli.rs` and `errors.rs` with only the `--root`, `init`, and `diagnose` behavior needed here, then implement root discovery from `--root`, `SUZUMUSHI_ROOT`, and `./suzumushi`.
2. [x] Implement `suzumushi init <path>` with canonical-root validation and fail-closed behavior for symlinked or non-empty unrelated destinations. Create app-private `state/`, `backups/`, and `logs/` as `0700`, private files as `0600`, and verify existing ownership/modes before use.
3. [x] Write default `config.toml`.
4. [x] Create `audio/playlists/demo/README.txt` with empty placeholder instructions.
5. [x] Create `audio/lyrics/README.txt` with sidecar and shared lyric instructions.
6. [x] Create `paths.rs` with root, audio, library, playlists, lyrics, state, artwork cache, logs, and backups paths.
7. [x] Implement supported audio extension detection.
8. [x] Implement one app-owned descriptor-rooted `scan/` traversal of `audio/` using `rustix` directory descriptors and the documented `openat2`/component-wise secure-open policy; do not invoke a pathname walker or a second traversal for library, playlists, lyrics, sidecars, or artwork.
9. [x] Classify `audio/library/` recursively without assigning special playback semantics to user folder names.
10. [x] Classify child folders of `audio/playlists/` as playlists during the same traversal.
11. [x] Treat file symlinks as non-descending entries and support them only through the bounded secure-open adapter after resolving/opening a verified regular target from the pinned parent. External targets are read/play-only; an unsupported kernel/filesystem security primitive returns `unsupported_secure_open` with no path-based fallback.
12. [x] Ignore directory symlinks and record every mount-boundary decision under configured counters.
13. [x] Pair same-stem `.lrc` sidecars with audio files.
14. [x] Index `audio/lyrics/` as a shared lyrics source.
15. [x] Detect sidecar artwork candidates such as `cover.jpg`, `cover.png`, `folder.jpg`, and `folder.png`.
16. [x] Read lightweight metadata needed for search, including artist, album artist, album, and title.
17. [x] Emit filename/path fallback search fields when metadata is missing.
18. [x] Add scan bounds for every encountered entry/directory/symlink/special file, file count, depth, playlists, entries per playlist, symlink resolutions, parser attempts, mount boundaries, per-path and aggregate path bytes, metadata fields/total, warning bytes, and one inclusive complete-index budget reserved alongside the old index under the process-wide budget.
19. [x] Build separate canonical `MediaAsset` and contextual `TrackEntry` models with defined stable-within-path IDs and scan generation.
20. [x] Enforce the exact root precedence `--root`, then `SUZUMUSHI_ROOT`, then existing `./suzumushi`, then error; an invalid selected higher-priority source does not fall through.
21. [x] Implement lock primitives and verified stale-lock handling. The per-user global lock uses only the verified current-UID `$XDG_RUNTIME_DIR/suzumushi/` location with no shared-`/tmp` fallback; the root lock stays descriptor-relative beneath the canonical root. Acquire the per-user active-TUI lock and then the canonical-root single-writer lock only when mutable TUI mode starts in Phase 3. Document and test that two concurrent login sessions for the same UID intentionally contend. Phase 2 `diagnose` and later status commands remain bounded read-only readers and acquire neither writer lock. Any future standalone mutating command acquires an explicit root mutation lease without claiming the global MPRIS/TUI identity.
22. [x] Before a third-party metadata parser sees media, use the `security` skill to record the concrete parser and filesystem risks in a concise root `SECURITY.md`. Record the selected parser's bounded in-process contract in `docs/parser-isolation.md`; use a helper process only if the chosen API cannot enforce the required input/work bounds. Add dependency checks to `mise.toml` when these runtime dependencies arrive.
23. [x] Return structured scan warnings.
24. [x] Add tests with temporary roots, all root-source/conflict/error combinations, invalid higher-priority roots, duplicate assets across playlists, repeated entries, copied files, symlinked files, retargeted/broken/external symlinks, FIFO media/config/README entries, exact nested initialization error paths, parent-directory rename/substitution during enumeration, magic-link refusal, verified regular-descriptor refusal, no pathname re-open instrumentation, unsupported secure-open behavior, component-wise natural-order fixed vectors, reversible invalid-path-byte display, root-level hidden-directory budget exclusion, symlinked roots, non-empty init destinations, private-directory ownership/mode refusal, secure runtime-lock location and missing-runtime failure, same-UID concurrent-session lock contention, shared lyrics, artwork candidates, searchable metadata, every counter at limit and limit+1, mount boundaries, one-traversal instrumentation, simultaneous old/new index and scanner/parser scratch reservation, and ignored directory symlinks.
25. [x] Add phase-owned fuzz targets for scanner path classification, config, lightweight metadata adapter input, and terminal-safe display projection. PR smoke runs use pinned total-time, per-input timeout, input-length, and RSS bounds; helper-backed parsers also fuzz bounded IPC framing and crash/timeout cleanup.

#### Done when

- [x] `suzumushi init ./suzumushi` creates the expected tree.
- [x] Demo playlist instructions are created.
- [x] Shared lyrics instructions are created.
- [x] Scanner finds copied playlist files.
- [x] Scanner finds symlinked playlist files.
- [x] One canonical asset can retain multiple playlist entries and playlist-local lyric context.
- [x] Scanner finds shared `.lrc` files under `audio/lyrics/`.
- [x] Scanner finds artwork sidecars without decoding them on the scan hot path.
- [x] Scanner emits searchable metadata when tags exist.
- [x] Scanner emits filename/path fallback search fields when tags are missing.
- [x] Scanner ignores symlinked directories.
- [x] Hidden directory subtrees are not traversed when `ignore_hidden_audio` is true, and their descendants cannot consume scan budgets.
- [x] Broken symlinks produce warnings.
- [x] FIFO media targets, config files, and expected README entries fail or warn without waiting for another process to open them.
- [x] Natural ordering compares nested path components independently before the exact raw-path tie-breaker.
- [x] Invalid Linux filename bytes use reversible terminal-safe escapes that cannot collide with literal escape text.
- [x] Instrumentation proves one descriptor-rooted enumeration with no accumulated-path reopen; parent substitution cannot redirect a scan, and unsupported secure symlink opens fail with a bounded warning.
- [x] No scan panic occurs from bad paths.
- [x] Lock primitive tests prove that two mutation leases cannot own one canonical root; actual mutable TUI acquisition is a Phase 3 acceptance criterion.
- [x] Lock primitive tests prove that only one mutable TUI lease can exist for the OS user across different login sessions and roots; Phase 2 diagnostics and read-only status commands remain concurrent and do not claim that lease.
- [x] Root precedence is identical for TUI, diagnostics, backups, and status commands.

#### Do not build yet

- Audio playback.
- Tag or lyric editing writes.
- Live file watching.

---

### Phase 3 - Minimal TUI Shell

**Status:** `not started`

**Goal:** Open a real `ratatui` interface with panels, focus, resize handling, and clean quit.

**Why this comes next:** Every feature needs reliable layout, input, redraw, and terminal cleanup.

**Expected result:** `suzumushi --root ./suzumushi` opens a placeholder TUI and `q` exits cleanly.

#### Build steps

1. [ ] Add only the exact minimal TUI/runtime/logging dependencies used by this phase.
2. [ ] Configure bounded file logging before entering the TUI; never write logs to the active terminal surface.
3. [ ] Create `terminal.rs` with raw mode and alternate screen cleanup.
4. [ ] Before terminal or MPRIS initialization, verify the private current-UID runtime directory and acquire locks in the documented order: per-user global active-TUI lock, then canonical-root single-writer lock. Refuse insecure/missing runtime storage and a second mutable TUI clearly, including one in another login session for the same UID; verify stale-lock process identity and release both on every normal/error/unwinding-panic path. Document that abort, `SIGKILL`, and power loss cannot run destructors and require stale-lock/terminal recovery on the next start.
5. [ ] Query terminal image protocol capability after entering alternate screen with a strict bounded timeout and cancellation path, but do not render art yet. Missing replies, pure Wayland, and tmux/Zellij without passthrough select fallback without delaying startup indefinitely.
6. [ ] Create `app.rs` with `AppState`.
7. [ ] Create `event.rs` for keyboard, resize, tick, and app events.
8. [ ] Create `input.rs` for keybindings, including the `Space` leader rule: a second chord key within the bounded, configurable `leader_timeout_ms` (default 250 ms) fires the chord; timeout expiry with no second key fires play/pause.
9. [ ] Create `ui/layout.rs` and `ui/status.rs`.
10. [ ] Draw placeholder panels for Library, Now Playing, Queue, Art, and Lyrics.
11. [ ] Add a bottom status/help bar.
12. [ ] Support `q` to quit.
13. [ ] Support `Tab` and `Shift+Tab` to cycle focus.
14. [ ] Support terminal resize without panic.
15. [ ] Add small-terminal fallback.
16. [ ] Add first `insta` snapshots at 80x24 and 120x32.

#### Done when

- [ ] TUI opens in alternate screen.
- [ ] Exit restores the terminal.
- [ ] Resize works.
- [ ] Focus changes are visible.
- [ ] Image protocol capability is recorded without breaking terminals that do not answer queries.
- [ ] Logs do not corrupt the TUI.

#### Do not build yet

- Real audio.
- MPRIS.
- Tag or lyric editing forms.

---

### Phase 4 - Library, Search, Playlist Browser, And Queue

**Status:** `not started`

**Goal:** Render scanned music, open `/` search, and build playable queues from folders, search results, and playlists.

**Why this comes before audio:** Queue and search-result semantics should be deterministic and tested before the audio worker consumes them.

**Expected result:** User can browse Library and Playlists, press `/`, type an artist/song/album/playlist, select a result, choose replace vs append, and see the ordered queue.

#### Build steps

1. [ ] Create `model/asset.rs`, `model/entry.rs`, `model/playlist.rs`, and `model/queue.rs`.
2. [ ] Create `search/mod.rs` with `SearchIndex`, `SearchEntry`, and grouped results.
3. [ ] Convert scanner output into app-owned models.
4. [ ] Build the first in-memory search index from scanned metadata and filename fallbacks.
5. [ ] Render Library and Playlists views.
6. [ ] Render `/` audio search overlay.
7. [ ] Support live filtering as the user types.
8. [ ] Group search results into Artists, Songs, Albums, and Playlists.
9. [ ] Support natural sorting.
10. [ ] Support `Enter` to prompt for replace vs append from folders, playlists, and search results.
11. [ ] Support `a` to append selected item to queue directly.
12. [ ] Support `v` to mark and unmark tracks for later batch actions.
13. [ ] Support `d` to remove queue item.
14. [ ] Support `c` to clear queue after confirmation.
15. [ ] Implement stable `QueueItemId`-based ordered play order, current item, and history; never retain indexes into the mutable `ordered` vector.
16. [ ] Implement `QueueMode::Shuffled` as seeded ID-based `play_order` plus explicit playback history; do not rewrite or deduplicate queue entries.
17. [ ] Implement repeat modes: off, one, queue.
18. [ ] Make the app loop the sole queue owner and increment `queue_generation` exactly once after each committed mutation. Queue generation validates mutations but never invalidates active playback events, which match item plus playback generation.
19. [ ] Implement the documented current-item transition table for append, repeat, shuffle, remove, clear, replace, and reorder while playing/paused.
20. [ ] Preflight every single and expanded replace/append/remove/clear/reorder operation with checked item-and-byte accounting against `queue_max_items = 10000` and `queue_max_bytes = 48 MiB` defaults before commit.
21. [ ] Add queue and search tests for zero, one, many, duplicate asset entries, stale scan selections, artist result expansion, album result expansion, song result selection, playlist result selection, replace prompt, append behavior, every current-item transition, shuffle toggles around the current item, actual ID-based previous-history behavior, repeat boundaries, item/byte limit and limit+1 atomic refusal, generation increments, and rescan generations.

#### Done when

- [ ] Playlists display from folders.
- [ ] `/` opens audio search.
- [ ] Artist, song, album, and playlist searches return grouped results.
- [ ] Search falls back to filename/path when tags are missing.
- [ ] Queue order matches natural folder order.
- [ ] Enter prompts before replacing or appending from browser or search results.
- [ ] `a` appends directly.
- [ ] Marked tracks are visible for later batch editing.
- [ ] Shuffle creates a stable inspectable queue.
- [ ] Previous works in shuffled mode.
- [ ] Repeat behavior is covered by tests.
- [ ] Limit failure leaves queue contents, history, current item, byte accounting, and generation unchanged.

#### Do not build yet

- Audio playback.
- Lyrics display.
- File writes.

---

### Phase 5 - Audio Playback Worker

**Status:** `not started`

**Goal:** Play local audio files from the queue without blocking the TUI.

**Why this comes after the queue:** Playback should consume a tested queue model instead of inventing state inside the audio layer.

**Expected result:** Selecting a playlist starts playback, Space pauses/resumes, and the UI shows current track state.

#### Build steps

1. [ ] Write one short audio ADR with the exact candidate crate versions/features and the required decode, seek, source-position, pitch-preserving speed, visualizer-tap, device, and shutdown behavior.
2. [ ] Try candidates one at a time. Record licenses, native/FFI implications, unsafe dependency boundaries, MSRV, and the Linux audio backend/features that would ship. Use the `security` skill for the real decoder/dependency boundary and extend the dependency checks in `mise.toml`.
3. [ ] With a few licensed fixtures and a deterministic no-device sink, prove the candidate path can decode advertised formats, preserve pitch and source position at `0.5x`, `1.0x`, and `2.0x`, reset on seek, expose the post-volume PCM tap, stay bounded, end once, and shut down cleanly. Add one short real-device smoke test, including pure Wayland without XWayland.
4. [ ] If no candidate satisfies the required behavior, change scope before building the worker. Remove rejected dependencies and pin the one selected path; do not preserve alternate production pipelines.
5. [ ] Create `audio/mod.rs`, `audio/command.rs`, and `audio/event.rs`, then create the worker that owns the output stream/player.
6. [ ] Implement the selected decode/output ownership model at fixed `1.0x`, with the selected stretcher bypassed.
7. [ ] Accept only `LoadTrack` with a bounded `QueueItemId` snapshot selected by the app loop; open and revalidate its file identity, then decode it off the UI path.
8. [ ] Make the UI the sole playback-generation allocator. Attach item plus playback generation to active commands/events, retain queue generation at load for diagnostics only, and reject stale playback identity before state changes.
9. [ ] Send loaded/state/ended/error over the reliable control channel, latest position over a capacity-one coalescing channel, and no visualizer traffic on either channel; seek acknowledgements join the reliable channel when seek is introduced in Phase 6.
10. [ ] Emit one `TrackEnded` after the final audible sample; the app loop, not the worker, resolves repeat/shuffle/history and sends the next `LoadTrack`.
11. [ ] Handle unsupported or corrupt files as visible errors.
12. [ ] Add worker play, pause, and stop commands; keep play-pause resolution and all next/previous/queue mutation actions in the app loop. Variable speed, seek, volume, and mute commands arrive only in Phase 6.
13. [ ] Add progress bar based on coalesced playback position.
14. [ ] Add focused tests around UI-owned playback transitions, stale generations, position coalescing, exact-once end events, shutdown, device loss, and source-position monotonicity without duplicating the pipeline selection cases.

#### Done when

- [ ] A real local file can play.
- [ ] One audio path is selected, pinned, and documented; rejected candidate dependencies are gone.
- [ ] The TUI remains responsive while music plays.
- [ ] Pause and resume work.
- [ ] Next, previous, repeat, and shuffle work through UI-owned queue transitions and selected-item snapshots.
- [ ] Decoder errors do not crash the app.
- [ ] Playback position is generated by the audio path, not guessed by a separate UI timer.

#### Do not build yet

- User-facing speed controls.
- Volume, mute, and seek controls.
- Mini visualizer.
- Lyrics sync.
- Desktop integration.

---

### Phase 6 - Playback Controls, Pitch-Preserving Speed, Mini Visualizer, And State Files

**Status:** `not started`

**Goal:** Add volume, seek, shuffle, repeat, pitch-preserving speed, a small audio-reactive visualizer, and stable now-playing state.

**Why this is core:** Suzumushi should feel like a real player before desktop integrations are layered on top, and v1 requires speed changes without pitch changes.

**Expected result:** User can control volume, speed, seeking, shuffle, and repeat from the TUI, see a subtle mini visualizer react to real playback samples, and have `state/now-playing.json` update atomically.

#### Build steps

1. [ ] Implement volume clamp and steps.
2. [ ] Implement mute/unmute.
3. [ ] Implement speed clamp and steps from `0.5x` to `2.0x`.
4. [ ] Integrate the selected pitch-preserving time-stretching path behind `dsp/` with fixed buffer capacities.
5. [ ] Bypass the stretcher at `1.0x`.
6. [ ] Reset stretch state on seek and track change.
7. [ ] Implement seek forward/backward.
8. [ ] Add the audio-worker seek, volume, `SetMuted`, and variable-speed commands defined by the final protocol. Transition commands carry expected/new UI-allocated playback generations; ordinary controls carry the current generation and reliable acknowledgments.
9. [ ] Wire the `s` keybinding to the Phase 4 shuffle queue model; Phase 4 already implements the play-order switching, so this phase only connects the control.
10. [ ] Wire the `r` keybinding to the Phase 4 repeat queue model in the same way.
11. [ ] Create a `NowPlayingState` model with PID, process start ticks, writer-instance ID, current boot ID, suspend-aware `CLOCK_BOOTTIME` update/position samples, diagnostic Unix time, muted state, queue/playback generations, and complete field-change trigger rules.
12. [ ] Create `visualizer/mod.rs` for RMS/peak sample reduction.
13. [ ] Generate bounded `VisualizerFrame` events from decoded playback samples.
14. [ ] Add `V` to toggle the mini visualizer.
15. [ ] Render the visualizer inside Now Playing with idle states for pause, stop, and silence.
16. [ ] Write one bounded, versioned `suzumushi/state/now-playing.json` by no-follow same-parent atomic replacement with user-only permissions and pinned parent identity. Feed it through the capacity-one latest-state writer, coalesce field-changing events under `status_write_coalesce_ms`, perform no progress heartbeat, and await the final stopped projection before lock release. Because the projection is reconstructible, do not `fsync` its temporary file or parent; missing post-crash state is a documented safe fallback, not data loss.
17. [ ] Keep state and visualizer updates bounded to avoid excessive disk writes or redraws. Renderers extrapolate playing position from the last acknowledged source-position/`CLOCK_BOOTTIME` sample with checked arithmetic. Tests prove an idle playing track performs zero periodic status writes, rapid queue/key activity coalesces to the latest generation, and wall-clock changes do not affect position or liveness.
18. [ ] Add focused deterministic injected-boot-clock/synthetic tests for sample/frame accounting, source-position extrapolation and clamping, seek buffer reset, mute volume restoration, UI-owned generation transitions, stale-event rejection, visualizer frame caps, and bounded state schema handling. Do not duplicate cases already proven while selecting the audio path.

#### Done when

- [ ] Volume cannot exceed configured bounds.
- [ ] Speed cannot exceed configured bounds.
- [ ] `0.5x` and `2.0x` preserve pitch.
- [ ] The authoritative source clock preserves the selected audio contract at every required speed; later lyric, MPRIS, and status work validates only consumer projections against it.
- [ ] `1.0x` bypasses the stretcher.
- [ ] Seek handles start/end boundaries.
- [ ] Visualizer bars react to real playback samples.
- [ ] Paused, stopped, muted, and silent playback render calm idle or flat states.
- [ ] Visualizer frames are dropped rather than buffered if the UI lags.
- [ ] Every documented field-changing event updates the latest in-memory now-playing projection; the capacity-one writer coalesces it under the maximum cadence, while documented immediate transitions and graceful shutdown flush promptly.
- [ ] State file writes are atomic.
- [ ] Steady playback causes no heartbeat writes or status `fsync`; status position remains correct through renderer-side boot-clock extrapolation.
- [ ] State is single-source, bounded, versioned, and safe under a second status-command process.

#### Do not build yet

- Waybar/tmux/Zellij commands.
- MPRIS.
- File editing.
- Full-screen or FFT spectrum visualizers.

---

### Phase 7 - Synced `.lrc` Lyrics And Local Lyric Editing

**Status:** `not started`

**Goal:** Parse local `.lrc` files, keep lyrics synced with playback, and allow safe local lyric edits.

**Why this comes after playback state:** Lyrics must follow the real playback position, not a separate timer. Editing should use the same parser that playback uses.

**Expected result:** An audio item with a sidecar or shared `audio/lyrics/` `.lrc` file shows synced lyrics, and the user can edit or create a local `.lrc` safely.

#### Build steps

1. [ ] Create `lyrics/mod.rs` and `lyrics/lrc.rs`.
2. [ ] Implement bounded `.lrc` loading.
3. [ ] Parse `[mm:ss.xx]` timestamps.
4. [ ] Support multiple timestamps per line.
5. [ ] Sort parsed lines by timestamp.
6. [ ] Resolve active line from playback position.
7. [ ] Implement polished sidecar lookup beside playlist entries and canonical targets.
8. [ ] Implement shared lookup under `audio/lyrics/`.
9. [ ] Detect ambiguous shared lyric filename matches and report a warning instead of guessing.
10. [ ] Wire active lyrics into the Now Playing and Lyrics panels.
11. [ ] Implement the shared `backups/` journal/manifest foundation with checked budgets and generic list/verify/previewed-prune plus the separately specified destructive restore workflow before enabling lyric writes.
12. [ ] Prove the separate new-file, atomic-replacement, full-file-tag, and restore journal paths plus startup dispositions. Cancellation is allowed only before commit, mutation acknowledgments are operation-ID reliable, no mutation task detaches, and the root lock remains held past normal worker timeout until durable completion or a recorded recovery disposition.
13. [ ] Create `lyrics_editor/` with a simple timestamped-line editor.
14. [ ] Support creating a new sidecar `.lrc` when no lyrics exist using a verified-absent preimage journal marker rather than a fictional backup; symlinked entries require an explicit playlist-sidecar versus canonical-target-sidecar choice.
15. [ ] Preserve valid UTF-8 original bytes, newline style, trailing whitespace, unsupported lines, blank lines, and final-newline state through source-referenced semantic nodes; invalid UTF-8 is non-editable and never lossy-written.
16. [ ] Validate timestamps before save.
17. [ ] Preflight lyric backup byte/entry budgets, revalidate symlink and file identity, and create/verify a journaled backup under `suzumushi/backups/lyric-edits/` for existing files.
18. [ ] Save through the pinned same-directory descriptor, flush/sync, descriptor-relative `renameat2`, and parent-directory sync while preserving permissions where supported; fail explicitly if atomic semantics are unsupported.
19. [ ] Reload and verify the lossless bounded LRC document after successful save.
20. [ ] Add tests for empty files, LF/CRLF, trailing whitespace, with/without final newline, invalid UTF-8 refusal, malformed timestamps, duplicate/multiple timestamps, byte-exact unchanged unsupported lines/metadata, long files, shared/ambiguous lookup, external-target refusal, retargeted symlink refusal, editing validation, verified-absent creation, backup byte/entry limit+1, list/verify/restore/previewed-prune, restore current-target preservation and budget failure, backup/disk/permission failure, every journal/save interruption point and disposition, shutdown during commit, startup recovery, and seek behavior.
21. [ ] Add phase-owned fuzz targets for LRC parsing/round-trip, mutation journal/manifest parsing, and bounded startup recovery dispatch; extend the pinned PR smoke corpus before lyric writing is enabled.

#### Done when

- [ ] Lyrics display for a local `.lrc` sidecar.
- [ ] Lyrics display from `audio/lyrics/` when no sidecar exists.
- [ ] Seeking updates active lyric immediately.
- [ ] Pause does not advance lyrics.
- [ ] Speed changes still use playback position correctly.
- [ ] User can create and edit local `.lrc` files.
- [ ] Bad `.lrc` files do not crash the app.
- [ ] Invalid lyric edits do not overwrite the existing file.
- [ ] Unchanged valid UTF-8 LRC content round-trips byte-for-byte, including newline and trailing-whitespace details; invalid UTF-8 is never overwritten.
- [ ] Backup/recovery operations exist and are proven before lyric editing is enabled.

#### Do not build yet

- Online lyric lookup.
- Lyric translation.
- Automatic lyric generation.

---

### Phase 8 - Album Art Pipeline And Terminal Display

**Status:** `not started`

**Goal:** Extract, cache, and display album art in terminals that support images, with safe fallback everywhere else.

**Why this comes before MPRIS art and notifications:** Desktop outputs need a stable local image path. The TUI also needs capability-aware rendering before release polish.

**Expected result:** Tracks with embedded or sidecar artwork show art in supported terminals and a calm fallback in unsupported terminals.

#### Build steps

1. [ ] Create `artwork/mod.rs`.
2. [ ] Read embedded artwork through metadata where supported.
3. [ ] Read sidecar artwork files such as `cover.jpg`, `cover.png`, `folder.jpg`, and `folder.png`.
4. [ ] For symlinked playlist entries, check artwork beside the canonical target.
5. [ ] Decode images off the UI thread.
6. [ ] Bound source bytes, width, height, total pixels, decoded RGBA bytes, and cache size before large allocation. Complete the selected image decoder's `docs/parser-isolation.md` gate first; an API that may perform unbounded in-process work moves behind the bounded helper contract.
7. [ ] Normalize cached artwork under `suzumushi/state/artwork/`; implement the versioned manifest, exact-byte admission, deterministic generation/hash eviction, MPRIS pin, bounded notification leases, text-fallback transition, startup orphan cleanup, and no-over-budget behavior from the Artwork Model.
8. [ ] Render artwork with `ratatui-image` when supported.
9. [ ] Render text fallback when no protocol is available.
10. [ ] Add `A` to toggle the art panel.
11. [ ] Add tests for lookup order, cache keys, compressed/decode/dimension/pixel limit+1 refusal, malicious headers, cache symlinks, missing art, cancellation, deterministic eviction order and hash tie-break, use-generation wrap, corrupt-manifest rebuild, startup orphan cleanup, expired/pre-reboot notification leases, MPRIS and notification pins, pin-pressure fallback transitions, and exact cache-budget accounting.
12. [ ] Add the selected image-header/decoder-adapter fuzz target, including helper IPC and cleanup when applicable, before advertising artwork support.

#### Done when

- [ ] Embedded art can be extracted when supported.
- [ ] Sidecar art can be found.
- [ ] Cached art path is stable for the current track.
- [ ] Eviction never removes an advertised MPRIS path or live notification lease, is deterministic for identical manifests, and never exceeds `max_cache_bytes`.
- [ ] TUI renders art or a fallback without corrupting the terminal.
- [ ] Artwork decode errors are visible warnings, not crashes.

#### Do not build yet

- MPRIS album art.
- Notification images.
- Embedded art editing.

---

### Phase 9 - Waybar, Tmux, And Zellij Status Outputs

**Status:** `not started`

**Goal:** Expose current playback state to Waybar, tmux, and Zellij through fast commands.

**Why this comes after now-playing state and artwork:** Status commands should read the one existing shared state file and exit. They should not know how to scan, decode, or play music.

**Expected result:** `suzumushi waybar --root ./suzumushi` prints Waybar JSON, `suzumushi tmux --root ./suzumushi` prints one compact tmux-safe line, and `suzumushi zellij --root ./suzumushi` prints one compact Zellij-status-safe line.

#### Build steps

1. [ ] Create `status/waybar.rs`.
2. [ ] Create `status/tmux.rs`.
3. [ ] Create `status/zellij.rs`.
4. [ ] Convert `NowPlayingState` into Waybar JSON.
5. [ ] Convert `NowPlayingState` into compact tmux text.
6. [ ] Convert `NowPlayingState` into compact Zellij text.
7. [ ] Consume the single state writer implemented in Phase 6; do not add a second writer or duplicate persisted representation.
8. [ ] Implement `suzumushi waybar --root <path>` as a pure renderer from shared state.
9. [ ] Implement `suzumushi tmux --root <path>` as a pure renderer from shared state.
10. [ ] Implement `suzumushi zellij --root <path>` as a pure renderer from shared state.
11. [ ] Add playing, paused, stopped, muted, lyrics, no-lyrics, and error classes for Waybar, matching the Waybar Model list exactly.
12. [ ] Add plain-text-safe tooltip formatting and document Waybar `escape = true`.
13. [ ] Add documentation snippets for Waybar, tmux, and `zjstatus`.
14. [ ] Add tests for schema versions, PID reuse/process-start/writer-instance/boot identity, future and pre-reboot boot-clock samples, forward/backward Unix-clock jumps, checked playing-position extrapolation, every state-field update trigger, zero steady heartbeats, queue/playback generations, mute versus zero volume, graceful stopped-state shutdown, oversize/corrupt/stale/symlink state, valid single-line JSON, and bounded terminal/format-control-safe tmux/Zellij text.

#### Done when

- [ ] Waybar command exits quickly.
- [ ] Tmux command exits quickly.
- [ ] Zellij command exits quickly.
- [ ] Missing state files return calm stopped states.
- [ ] Corrupt state files return safe fallback output.
- [ ] Oversized, unsupported-version, stale, or symlinked state fails to bounded safe output without blocking.
- [ ] Only `now-playing.json` is persisted; derived status outputs cannot drift.
- [ ] Waybar JSON is generated through `serde_json`.
- [ ] Tmux output is one sanitized line.
- [ ] Zellij output is one sanitized line.

#### Do not build yet

- MPRIS.
- Waybar click actions.
- A custom Zellij WASM plugin.
- A separate widget daemon.

---

### Phase 10 - MPRIS, Media Keys, Notifications, And MPRIS Album Art

**Status:** `not started`

**Goal:** Expose Suzumushi through Linux MPRIS, support media keys and `playerctl`, publish album art, and notify on track changes.

**Why this comes after internal commands, shared state, and artwork:** Desktop integration should translate into existing app actions and publish existing metadata/artwork, not create separate playback state.

**Expected result:** `playerctl -p suzumushi play-pause`, desktop media keys, desktop media widgets, MPRIS album art, and track-change notifications work while Suzumushi is running.

#### Build steps

0. [ ] Verify `souvlaki` exposes every field in the MPRIS Model table (`albumArtist`, `trackNumber`, `genre`) and supports per-generation `SetPosition` filtering; if any is missing, adopt `zbus` directly. Record the decision as an ADR before any other Phase 10 step.
1. [ ] Create `desktop/mpris.rs`.
2. [ ] Create `desktop/notifications.rs`.
3. [ ] Start MPRIS registration when TUI mode starts.
4. [ ] Verify and reuse the per-user global active-TUI lock already acquired at mutable TUI startup before registration; do not acquire a second lock or create another ownership path.
5. [ ] Publish player identity as Suzumushi.
6. [ ] Publish playback status.
7. [ ] Publish metadata for active track, including `mpris:artUrl` when cached art exists.
8. [ ] Map MPRIS Play/Pause/PlayPause/Stop/Next/Previous to app actions.
9. [ ] Map Seek and SetPosition if reliable.
10. [ ] Ignore stale `SetPosition` calls for old track IDs or playback generations; a queue-only edit does not invalidate positioning of the same active item.
11. [ ] Update MPRIS metadata on track change, tag edit, lyric source change, and artwork cache update.
12. [ ] Send desktop notification on track change only.
13. [ ] Include cached artwork path in notifications when available.
14. [ ] Debounce fast track changes to avoid notification spam.
15. [ ] Handle missing D-Bus session or notification daemon as warnings.
16. [ ] Add tests around global/per-root lock ordering and stale recovery, concurrent status commands, nonblocking D-Bus admission, command mapping through app actions, stale playback/track rejection, acceptance after unrelated queue edits, metadata generation, notification throttling, and art URL formatting.

#### Done when

- [ ] `playerctl -l` can see Suzumushi on a normal desktop session.
- [ ] Media keys control play/pause/next/previous.
- [ ] Metadata updates when tracks change.
- [ ] MPRIS album art appears where desktop widgets support it.
- [ ] Track-change notifications appear with art when available.
- [ ] D-Bus or notification failure does not stop playback.

#### Do not build yet

- Background daemon mode.
- Notification action buttons.
- Remote network control.

---

### Phase 11 - Safe Single-Track And Batch Tag Editing

**Status:** `not started`

**Goal:** Edit common tags for one or many tracks with preview, confirmation, configured backups, and post-write rescan.

**Why this comes late:** Tag editing mutates user files. It should be added only after path handling, metadata display, backup paths, queue state, and desktop metadata updates are mature.

**Expected result:** User can edit one track or a marked batch, preview every changed field, confirm, create verified full-file backups, write tags, rescan changed tracks, and update visible metadata.

#### Build steps

0. [ ] Before editor code, use licensed scratch copies to prove the exact tag adapter can use verified descriptors, perform same-parent atomic replacement, preserve unedited audio and metadata, reopen/verify, and restore after failed verification. Record the supported combinations in `docs/tag-mutation-compatibility.md`; unsupported combinations remain read-only.
1. [ ] Create `tags/mod.rs` if not already present from scan metadata work.
2. [ ] Use `lofty` to read common tag fields.
3. [ ] Normalize tag fields into `TrackTags`.
4. [ ] Create single-track tag editor UI state.
5. [ ] Create batch tag editor UI state.
6. [ ] Add editable fields for title, artist, album, album artist, track number, disc number, genre, year, and comment.
7. [ ] For batch edits, apply only fields explicitly selected in the batch editor.
8. [ ] Validate field lengths and numeric fields.
9. [ ] Build a diff preview for one track and every file in a batch.
10. [ ] Refuse to edit any file currently loaded/open by the audio worker, including paused state, until a matching unload acknowledgment.
11. [ ] Deduplicate a batch by verified physical file identity, reject conflicting edits, and refuse it if any target is currently loaded/open by the audio worker.
12. [ ] Resolve symlink targets and show the target before save.
13. [ ] Capture stable file identity plus size/time/content hash before preview and revalidate the symlink and opened target before backup and write.
14. [ ] Refuse targets outside the canonical Suzumushi root and all targets with link count other than one; external symlink targets and hard-linked files are read/play-only in v1.
15. [ ] Reuse the Phase 7 journaled backup/list/verify/restore/previewed-prune and recovery implementation; do not create a tag-only backup subsystem.
16. [ ] Create and verify a full-file backup before every write by default; reuse only identical verified content-addressed backups.
17. [ ] Enforce backup byte budget, metadata/tag bounds, and max batch size.
18. [ ] Refuse write if any symlink or target identity/content changed after preview.
19. [ ] Write tags only through an adapter using the already verified descriptor and pinned parent contract, with same-parent atomic replacement; do not fall back to Lofty's path-reopening convenience API or a non-atomic format/library path. Unsupported formats fail explicitly.
20. [ ] Reopen and parse the result, verify expected tags, stable audio properties, and exact preservation of all unedited metadata/pictures/attachments where the format supports inspection; restore and fail if this cannot be verified.
21. [ ] Stop on first write failure and show an honest partial batch summary.
22. [ ] Reopen and verify the post-write file identity, then atomically update every queued snapshot for the changed asset as one queue mutation before rescan; preserve queue item IDs/order while never retaining the obsolete expected identity.
23. [ ] Update search index entries, MPRIS, shared status state, notifications state, and visible queue metadata if edited assets are active or queued.
24. [ ] Add tests for validation, shared backup journal/recovery, every terminal disposition, verification/budget/prune/destructive restore, external/retargeted symlink refusal, hard-link refusal, changed-file refusal, verified-descriptor writes, preservation of unedited tags/pictures/metadata, disk/permission/interruption failure, shutdown during commit with root lock retained, post-write validation/restore, batch physical-target deduplication/conflicts, max batch size, loaded-file refusal, and partial failure summaries.
25. [ ] Add fuzz targets for the proven writable tag adapters and normalized tag strings, constrained to scratch copies and the same parser/helper limits. A crash, timeout, preservation mismatch, or leaked helper blocks that writable format.

#### Done when

- [ ] Single-track saving requires explicit confirmation.
- [ ] Batch saving requires explicit confirmation with file count and field list.
- [ ] A verified full-file backup is created before every write.
- [ ] Backup failure prevents writes.
- [ ] Any file currently loaded/open by the audio worker is refused until unload acknowledgment.
- [ ] Symlink target edits are obvious.
- [ ] Search results reflect edited metadata after rescan/update.
- [ ] UI and desktop metadata update after successful write.

#### Do not build yet

- Online tag lookup.
- Filename rewriting.
- Embedded album art editing.

---

### Phase 12 - Search Palette Polish, Diagnostics, And V1 Fit-And-Finish

**Status:** `not started`

**Goal:** Make Suzumushi feel complete enough for a serious v1, not a demo.

**Why this comes after core features:** Basic `/` search arrives earlier. This phase makes search feel release-grade and turns `Space+/` into the global palette without hiding missing foundations.

**Expected result:** User can use `/` for fast local-audio search, `Space+/` for audio plus commands, inspect warnings, use help, see clear empty states, use terminal art/fallbacks, tune the mini visualizer, and understand the active desktop/status integrations.

#### Build steps

1. [ ] Add `Space+/` global palette.
2. [ ] Add command results for rescan, shuffle, repeat, lyrics editor, tag editor, batch tag editor, art toggle, visualizer toggle, diagnostics, and quit.
3. [ ] Improve `/` ranking with exact, prefix, word-prefix, and fuzzy scoring.
4. [ ] Make metadata matches rank above filename-only matches.
5. [ ] Add highlighted match spans in search results if the UI remains readable.
6. [ ] Add empty search states and ambiguous search diagnostics.
7. [ ] Add scan diagnostics panel.
8. [ ] Add audio diagnostics panel.
9. [ ] Add visualizer diagnostics for frame rate, dropped frames, and disabled backend support.
10. [ ] Add desktop diagnostics for MPRIS, notifications, Waybar, tmux, Zellij, and image protocol support.
11. [ ] Add help overlay.
12. [ ] Implement the terminal-native theme using default foreground/background and named ANSI palette slots only.
13. [ ] Add `mono` mode and honor `NO_COLOR`.
14. [ ] Add config validation messages in the TUI.
15. [ ] Add empty-state screens for no music, no playlists, no lyrics, no art, no visualizer data, no audio device, no D-Bus, no notification daemon, and unsupported image protocol.
16. [ ] Add UI snapshots for common states.
17. [ ] Add a small fixture `examples/suzumushi/` root for demos if licensing is clean.

#### Done when

- [ ] `/` finds creators, titles, albums or series, playlists, filenames, and paths without leaving the keyboard.
- [ ] `Space+/` opens a global palette with local audio and commands.
- [ ] Metadata matches rank above filename-only matches.
- [ ] Diagnostics explain skipped files and integration failures.
- [ ] Help overlay documents all v1 controls.
- [ ] The default UI inherits custom terminal palettes and remains readable without configuration on representative light and dark themes.
- [ ] No UI style contains a hard-coded RGB color or relies on color as the only state indicator.
- [ ] `mono` mode and `NO_COLOR` remain fully usable.
- [ ] Album art fallback looks intentional, not broken.
- [ ] Mini visualizer fallback looks intentional, not random or broken.
- [ ] Empty/error states are calm and actionable.

#### Do not build in this phase

- Online metadata services.
- Live file watching.

Streaming-service connections and podcast/RSS download management are permanently outside the planned product scope, not deferred work.

---

### Phase 13 - Linux Release Candidate

**Status:** `not started`

**Goal:** Package, document, and verify Suzumushi as a Linux-only local audio player.

**Why this comes last:** Releasing before audio, lyrics, artwork, MPRIS, notifications, Waybar, tmux, Zellij, and tag editing are stable would create support pain.

**Expected result:** A user can install Suzumushi, create a root, drop music into folders, configure Waybar/tmux/Zellij, use media keys, see notifications, album art, and the mini visualizer, edit lyrics, edit tags safely, and understand the limits.

#### Build steps

1. [ ] Write README with install, init, folder layout, controls, lyrics, lyric editing, artwork, mini visualizer, Waybar, tmux, Zellij, MPRIS, notifications, and tag editing safety.
2. [ ] Add `suzumushi diagnose` output for root/config/audio/DSP/visualizer/lyrics/artwork/MPRIS/notifications/Waybar/tmux/Zellij checks.
3. [ ] Add and package shell completions through `clap_complete` for every supported install channel.
4. [ ] Audit `mise run ci` and `.github/workflows/ci.yml`. The task includes Rust 1.97.1 format, Clippy, locked tests/doc tests, the Rust 1.95.0 MSRV contract, dependency security, queue/root/global-lock concurrency, mutation faults, and bounded fuzz smoke.
5. [ ] Run the bounded deep fuzz, sanitizer, and long fault tasks manually before a release and after parser/mutation-sensitive changes. Retain and fix every crash, timeout, sanitizer, or preservation reproducer; do not create a scheduled workflow.
6. [ ] Add the only other workflow, `.github/workflows/release.yml`. It triggers on `v*` tags, verifies that the tag, `Cargo.toml`, and changelog version agree, installs pinned `mise`, runs the release checks, builds and smoke-tests the Linux release through `mise run release-build`, creates an archive plus SHA-256 checksum, and publishes one GitHub Release. Together with `ci.yml`, these are the only workflows.
7. [ ] Package the one canonical `suzumushi` executable. Linux installer/package instructions create and verify a relative `suzu -> suzumushi` symlink; archive and clean-host tests reject an absolute, dangling, or escaping alias.
8. [ ] Maintain only the prebuilt AUR package `suzumushi-bin` after each GitHub Release. Update its release URL/checksum manually and run the clean Arch install/upgrade/uninstall test; do not publish a source-building `suzumushi` AUR package or add another workflow.
9. [ ] Freeze `docs/support-matrix.md` before candidate QA. It names exact distro/release, kernel, libc, GNOME, KDE Plasma, wlroots compositor, terminal, multiplexer, notification daemon, and PipeWire/PulseAudio/ALSA versions; marks each combination required, sampled, or unsupported; and records the clean-host image/hardware identity. Run manual QA on the exact packaged digest in every required pure-Wayland combination with XWayland disabled, recording evidence, deviations, and verdict. The same exact digest must pass `SUZU-QA-NET-001` under blocked and syscall-observed IPv4/IPv6 egress before publication.
10. [ ] Document build dependencies introduced by audio, DSP, image, D-Bus, and system audio crates for `cargo install`, plus runtime dependencies for `suzumushi-bin` and native Linux binary artifacts.
11. [ ] In the same `release.yml`, package and inspect the generated `.crate`, install it on a clean Linux job, and publish it to crates.io only after the archive jobs pass. Then run `cargo install --locked suzumushi --version <tag-version>` as a post-publication smoke test; document yank/patch recovery.
12. [ ] Verify project domain and crates.io package ownership before presenting either as reserved.
13. [ ] Move the `Unreleased` changelog content into the `1.0.0` section and set the package version in the reviewed untagged release commit before building its candidate artifacts. Tag that unchanged commit `v1.0.0` only after playback, pitch-preserving speed, mini visualizer, lyrics, artwork, MPRIS, notifications, Waybar, tmux, Zellij, tag editing, security, and focused QA meet their documented contracts.

#### Done when

- [ ] Fresh install path is documented.
- [ ] Waybar snippet works.
- [ ] Tmux snippet works.
- [ ] Zellij `zjstatus` snippet works.
- [ ] Media keys work through MPRIS.
- [ ] MPRIS album art works with cached artwork.
- [ ] Desktop notifications work on track change.
- [ ] Mini visualizer behavior and fallback are documented.
- [ ] Lyric editing safety behavior is documented.
- [ ] Tag editing safety behavior is documented.
- [ ] CI is green.
- [ ] The exact release commit and lockfile pass `mise run security` with no unowned/unexpired exception.
- [ ] Release artifacts have checksums.
- [ ] The GitHub Release contains the versioned archive and its SHA-256 checksum, both built from the tagged source.
- [ ] `suzumushi-bin`, installer, and native Linux prebuilt flows install one canonical binary plus a verified relative `suzu` symlink; Cargo behavior is documented honestly and does not promise that package-created link.
- [ ] `CHANGELOG.md` documents every tagged release since `0.1.0`.
- [ ] Cargo, `suzumushi-bin`, and native Linux prebuilt channels each have a named external repository or registry integration, least-privilege publishing, checksum/hash updates, rollback ownership, and clean-host install/upgrade/uninstall evidence. A channel without this evidence is not advertised as shipped.
- [ ] Release QA follows the `qa` skill, names the exact packaged digest and environment, records focused user steps and evidence, and gives an explicit pass/fail/blocked/inconclusive verdict.
- [ ] No inconclusive release gate is treated as passing.
- [ ] `SUZU-QA-NET-001` observes no `AF_INET`/`AF_INET6` socket, connect, or datagram-send attempt from the exact packaged artifact across the full supported local workflow.
- [ ] The tagged commit has passed the manual bounded fuzz/sanitizer tasks with no unresolved finding, and every required support-matrix combination has an explicit verdict.

#### Do not build yet

- Cross-platform support.
- Streaming service plugins.
- Cloud sync.
- Remote control over the network.

---

## Testing Strategy

Follow the `test-quality` skill. Keep the suite as small as practical: test meaningful public behavior and credible failure boundaries, combine related cases in readable tables, and do not duplicate the same contract across unit, integration, and CLI layers. Do not test trivial getters, private helpers already covered through public behavior, or chase coverage numbers.

CI/release workflow contract:

- The single `ci.yml` runs `mise run ci` on every push and pull request. That task owns Rust 1.97.1 format, Clippy, locked tests/doc tests, the Rust 1.95.0 MSRV contract, root/global-lock concurrency tests, mutation faults, bounded fuzz smoke, and `cargo deny`/`cargo audit`. Reliable control-channel tests never depend on sleeps or retries.
- Deep fuzzing, sanitizers, longer fault campaigns, and packaged-artifact QA are explicit `mise.toml` tasks run manually before a release and after relevant high-risk changes. Reproducers and failure evidence are retained; no scheduler is required.
- The single `release.yml` accepts version tags only, runs the release tasks, builds and smoke-tests the Linux archive, publishes its checksum, and optionally publishes the verified `.crate` after the archive succeeds.
- If future application features exist, one reviewed release set remains authoritative. CI, QA, and package tasks explicitly select it; testing another combination is not a proxy for release behavior.

Config and bound tests:

- One table-driven unit case per row in the compiled configuration-range table accepts the exact minimum and maximum and rejects minimum-minus-one and maximum-plus-one where representable
- Cross-field cases cover every published invariant: speed range contains `1.0`, short seek does not exceed long seek, entries cover files, history does not exceed queue items, preferred artwork size fits both dimensions, and at least one search weight is non-zero
- Non-finite floats, integer overflow, zero where prohibited, unknown fields/sections, invalid enums, oversized/malformed key chords, and an `unlimited` spelling all fail before any worker, file open, or allocation
- Lower individually valid limits that cannot satisfy the process ledger fail preflight without partially starting the requested operation

Scanner tests:

- Empty root
- One audio file
- Nested library folders
- Playlist folders
- Copied playlist files
- Symlinked playlist files
- Broken symlinks
- Shared lyrics under `audio/lyrics/`
- Symlinked directory ignored
- Parent directory renamed/substituted during enumeration cannot redirect the pinned descriptor traversal
- No scanner path uses accumulated-path reopen; magic links and unsupported secure file-symlink opens produce their exact bounded warnings
- Unsupported extension ignored with warning
- Max files limit
- Max depth limit
- Per-path, aggregate-path, metadata, warning, entry/directory/symlink/parser/mount counters, and inclusive index byte limits at exact limit and limit+1; retained capacity accounting reserves old and replacement indexes plus all app-owned components within the 384 MiB ledger
- Root precedence for each source, every conflict, invalid selected source, and final error

Queue tests:

- Natural order
- Empty queue
- Single-track queue
- Next at end with repeat off
- Next at end with repeat queue
- Repeat one
- Previous in ordered mode
- Previous in shuffled mode
- Shuffle deterministic with seed
- Remove/reorder with duplicate entries preserves ID-based current/history meaning and never shifts another item into a stale reference
- Append/repeat/shuffle/non-current removal leaves matching active playback events valid
- Remove-current, clear, replace, and reorder follow the documented playing/paused transition table with exact-once stop/load behavior
- Checked item/byte expansion and mutation at limit and limit+1
- Queue generation increments exactly once only after commit
- UI is sole next/previous/repeat/shuffle and playback-generation owner; audio receives one selected identity snapshot

Search tests:

- Empty query state
- Artist metadata match
- Song title metadata match
- Album metadata match
- Playlist name match
- Filename fallback when tags are missing
- Relative path fallback when tags are missing
- Metadata match ranks above filename-only match
- Exact match ranks above fuzzy match
- Artist result expands to all artist songs
- Album result expands to all album songs
- Search result Enter prompts replace vs append
- Search result from a stale scan generation is rejected before queue construction
- `Space+/` includes command results
- Search output strips control characters
- Max result limit is enforced

Audio and DSP deterministic PR tests:

- Volume clamp
- Speed clamp
- `1.0x` bypasses time-stretching
- Seek resets time-stretch state
- One matching end-of-track event makes the UI advance the queue once and send one selected snapshot
- Decoder error becomes visible app error
- Fake backend keeps UI state deterministic
- Versioned synthesized fixtures and an independently specified estimator verify pitch/frame/source-position math without wall-clock assertions; intentionally broken fixture variants prove each oracle fails for the expected reason
- Device loss, seek, pause, end-of-track, and shutdown produce one ordered reliable transition per UI-owned playback generation; delayed acknowledgements remain authoritative, while coalesced position and visualizer telemetry must match the acknowledged `audio_state_revision` and increase only its own lane sequence
- Appending/removing/repeating/shuffling while a track remains active does not invalidate its matching playback events
- Reliable control/state events survive simulated channel pressure; a deterministic simultaneous-full command/event-lane case proves both actor loops continue draining and cannot await each other cyclically. External admission returns busy before mutation when the bounded outbox is full, position coalesces, and visualizer drops independently without blocking the fake real-time callback.

Visualizer tests:

- RMS levels generated from fake PCM samples
- Silence maps to zero or idle levels
- Levels are clamped to the fixed 0..=8 range
- Bar count is capped to the configured maximum
- Frame rate cap is enforced
- Stale visualizer frames are ignored after track change or seek generation change
- Full UI channels drop frames instead of buffering unbounded work
- Paused, stopped, and muted states render deterministic idle or flat output

Lyrics tests:

- Empty `.lrc`
- Single timestamp
- Multiple timestamps per line
- Duplicate timestamps
- Malformed timestamp
- Long line rejection
- Max file size rejection
- Sidecar lookup
- Shared lookup under `audio/lyrics/`
- Ambiguous shared lookup warning
- Active line after seek
- Active line while paused
- Create sidecar `.lrc`
- Edit shared `.lrc`
- Invalid lyric edit refuses save
- Lyric backup failure prevents write
- Atomic lyric save reloads active lyrics
- Valid UTF-8 unchanged content preserves LF/CRLF, trailing whitespace, unsupported lines, and final-newline state byte-for-byte
- Invalid UTF-8 is non-editable and never lossy-written
- Lyric backup byte/entry budgets enforce limit and limit+1

Artwork tests:

- Embedded art lookup
- Sidecar `cover.jpg` lookup
- Sidecar `folder.png` lookup
- Symlink target artwork lookup
- Oversized image refusal
- Artwork cache key stability
- Deterministic oldest-generation eviction with lexical hash tie-break
- MPRIS pin and bounded notification leases prevent live-path eviction without exceeding the byte budget
- Pin pressure performs the documented text-fallback transition; corrupt manifests, orphan temporaries, expired leases, and generation wrap recover deterministically
- Terminal image unsupported fallback
- MPRIS art URL uses cached file URI

Tag tests:

- Missing tags display as missing internally
- Common tags read correctly
- Control characters sanitized for display
- Edit validation catches invalid numbers
- Full-file backup failure prevents write
- Changed file prevents write
- Symlink target is shown before save
- Loaded/open file cannot be edited until unload acknowledgment
- Batch edit preview lists every affected file
- Batch edit applies only selected fields
- Batch edit deduplicates physical identities, rejects conflicts, and refuses loaded/open files
- Batch edit enforces max batch size
- Batch edit reports partial failure summary

Backup and mutation tests:

- Generic list/verify/previewed-prune plus destructive restore behavior is shared by lyrics and tags
- New-file, atomic replacement, full-file tag, and restore journal paths reach the correct `Completed`, `RolledBack`, `RecoveryRequired`, or `Unrecoverable` disposition after every injected interruption, malformed/truncated/checksum-invalid journals included
- Journal record bytes, aggregate bytes, entry counts, tag/lyric backup entries, and startup recovery attempts pass at limit and refuse limit+1 without deleting existing evidence or starting a mutation
- App-owned runtime/state/backup/journal/log directories and files enforce current-UID ownership and `0700`/`0600` modes; insecure existing storage fails closed and backups never broaden source access
- Restore preserves the current target first, fails closed when that budget is unavailable, and refuses loaded, external-target, or hard-linked files
- Cancellation succeeds only before commit and never detaches a mutation
- Shutdown during commit retains the root lock and waits beyond the normal worker timeout until durable completion/recovery
- Incomplete operations protect referenced backups from prune

Waybar tests:

- Stopped state JSON
- Playing state JSON
- Paused state JSON
- Error state JSON
- Single-line output
- Missing state file behavior
- Corrupt state file behavior
- Muted class derives from explicit `muted`, not volume zero
- PID reuse is rejected using process start ticks and writer-instance identity
- A current-boot `Active` record with matching process/instance/held-lock identity renders live data regardless of forward/backward Unix-clock changes
- A boot-ID mismatch, future `CLOCK_BOOTTIME` sample, PID/start mismatch, or absent/mismatched per-user lock renders the bounded error fallback
- Checked boot-clock extrapolation advances only playing position, honors `speed_milli`, clamps at duration, and leaves paused/stopped position unchanged
- An injected writer records zero periodic writes during steady playback; burst events coalesce within `status_write_coalesce_ms`
- Every documented state-field trigger updates the bounded state, including the last acknowledged monotonic `audio_state_revision`; graceful shutdown writes stopped state

Tmux tests:

- Stopped state text
- Playing state text
- Paused state text
- Single-line output
- Control characters are stripped
- Missing state file behavior
- Corrupt state file behavior

Zellij tests:

- Stopped state text
- Playing state text
- Paused state text
- Single-line output
- Control characters are stripped
- Missing state file behavior
- Corrupt state file behavior
- `zjstatus` documentation snippet uses the `suzumushi zellij` command

MPRIS tests:

- Play maps to app action
- Pause maps to app action
- PlayPause maps to app action
- Next/Previous map to app actions
- Stale SetPosition is ignored
- Metadata updates are generated from app state
- Album art URL updates from cached artwork
- Only one per-user mutable TUI can own the fixed identity across different login sessions and roots
- Per-root and global stale-lock recovery verifies identity; status commands remain concurrent
- Full external command channel returns a bounded D-Bus busy/error response without blocking or displacing accepted controls

Notification tests:

- Track change creates notification event
- Pause/seek/progress does not create notification event
- Fast next/previous is debounced
- Missing notification daemon is non-fatal
- Notification image uses cached artwork when available

Input and keybinding tests:

- Every key resolves to exactly one action per focused panel; a second binding for the same key and panel fails an explicit conflict check
- With an injected clock, `Space` alone fires play/pause exactly at `leader_timeout_ms` expiry, and a chord key one tick before expiry fires the chord instead
- `Enter`, `e`, `d`, `Tab`, and `Ctrl+s` dispatch by focused panel and never fire two actions from one press
- The help overlay lists exactly the bindings registered for the focused panel, so documented and dispatched controls cannot drift apart

TUI tests:

- 80x24 default layout snapshot
- 120x32 default layout snapshot
- Small terminal fallback
- Empty library screen
- Lyrics screen
- Lyric edit screen
- Album art fallback screen
- Mini visualizer playing and idle snapshots
- Tag edit preview screen
- Batch tag preview screen
- Error panel

### Executable QA Contract

Follow the `qa` skill for executable QA. `docs/testing.md` records only
release-critical manual cases with their command or user steps, fixture,
expected result, cleanup, evidence, and `PASS`, `FAIL`, `BLOCKED`, or
`INCONCLUSIVE` verdict. Automated tests need stable names, not a parallel QA case
register. Every stateful case creates a fresh root, uses deterministic fakes or
named physical test hardware, and verifies cleanup and lock release. No case
waits by sleeping or retries a product failure into green.

Required cases include `SUZU-QA-FS-001` (non-regular and retargeted symlink
refusal), `SUZU-QA-FS-002` (descriptor-relative atomic replacement or explicit
unsupported result), `SUZU-QA-SCAN-001` (every scan counter at its limit and
one beyond it), `SUZU-QA-MEM-001` (old/new index reservation within the process budget),
`SUZU-QA-AUDIO-001` (cross-channel telemetry cannot reject or regress acknowledged pause/mute/speed state),
`SUZU-QA-MUT-001` (journal corruption becomes recovery-required without panic),
`SUZU-QA-MUT-002` (full-file backup and preservation of untouched metadata),
`SUZU-QA-NET-001` (no runtime IPv4/IPv6 socket or egress attempt from the exact
packaged artifact), and `SUZU-QA-REL-001` (prebuilt and Cargo source-install channel
installation). Repeat a case only to investigate an intermittent result, and
record every outcome rather than retrying until green.

`SUZU-QA-NET-001` runs the exact candidate digest, without rebuilding, on an
isolated Linux QA host. The harness blocks external IPv4/IPv6 traffic and
observes the candidate process tree's `AF_INET`/`AF_INET6` socket creation,
`connect`, and datagram-send attempts while exercising init, scanning, local
playback, lyrics, artwork, status renderers, MPRIS/notifications, tag preview and
write/restore, and shutdown. Local `AF_UNIX` sockets required for D-Bus and audio
are allowlisted by family, not by disabling observation. Any network attempt is
a failure even when blocking prevents a packet from leaving; a harness that
cannot prove process-tree coverage is `INCONCLUSIVE`, never a pass.

Cross-cutting test rules:

- Inject clocks, RNG seeds, file identity, process identity, notification debounce, playback generations, and audio devices; never synchronize with sleeps.
- Inject state revisions plus independent position and visualizer sequences; assert reliable acknowledgements remain authoritative regardless of telemetry arrival order and stale/future-revision telemetry cannot regress accepted state.
- Inject and assert queue generations on optimistic queue mutations and persisted diagnostics; inject item plus UI-owned playback generations on active audio commands/events. Tests must prove that unrelated queue generation changes neither authorize stale mutations nor reject valid active playback events.
- Use property tests only where one clear invariant covers meaningful input space better than a small example table, such as natural ordering or LRC round-trips.
- Keep narrow fuzz targets only for untrusted parser and serialization boundaries introduced by the current phase. Retain minimized failures as regression fixtures; do not grow redundant corpora.
- Add fault injection only for credible failure points in the current phase, especially mutation commit/recovery and bounded channel admission.
- Test through public behavior and real seams. Every regression test is first observed failing against the broken behavior.
- Keep a small deterministic binary-level suite in `tests/cli.rs` that spawns the actual `suzumushi` executable for help, init, diagnose, backups, Waybar, tmux, and Zellij. It asserts exit status, stdout/stderr framing, durable files and permissions, forbidden side effects, child-process cleanup, and lock release; PTY-driven TUI startup/quit coverage exercises the public terminal guard without replacing focused library tests.
- Snapshot changes require a reviewer to explain the semantic UI change; generated snapshots nobody reviews should be deleted.
- Repeated parallel runs must be clean. A flaky audio/timing test is fixed or removed, never retried into green.

Manual QA:

```txt
exact packaged artifact digest recorded before QA
pure Wayland with XWayland disabled on every required exact GNOME, KDE Plasma, and wlroots combination in `docs/support-matrix.md`
selected PipeWire/PulseAudio/ALSA backend paths and missing-backend diagnostics
package-installed `suzu` relative symlink; Cargo install documents canonical-only behavior
fresh root init
copied songs
symlinked playlist songs
broken symlink warning
/ artist search
/ song search
Space+/ command palette
play mp3/flac/ogg/wav/m4a
pause/resume
seek
speed 0.5x and 2.0x
pitch-preserving speed
mini visualizer reacts to playback
mini visualizer becomes idle on pause/stop/silence
lyrics sync
shared lyrics lookup
local lyric edit and restore from backup
album art in supported terminal
album art query timeout and fallback without terminal/multiplexer protocol passthrough
Waybar module
tmux status output
Zellij status output through `zjstatus`
playerctl commands
physical media keys
missing D-Bus session, media-key binding, and notification daemon remain non-fatal
MPRIS album art
desktop notification on track change
safe tag edit and restore from full-file backup
batch tag edit with preview
second mutable instance refused while status commands still work
retargeted/external symlink mutation refusal and hard-link refusal
disk-full and permission-denied lyric/tag/state/artwork paths
backup list/verify/restore/previewed-prune
audio output device loss and recovery
shutdown during scan, artwork decode, lyric save, tag save, and playback; committed mutation retains root lock and recovers from its journal
oversized state/metadata/path/tag/artwork inputs at limit and limit+1
exact packaged artifact under `SUZU-QA-NET-001` blocked-and-observed IPv4/IPv6 egress
every advertised container/codec combination from the compatibility matrix
mono theme and no-color mode remain fully usable
small-terminal fallback message below 80x24
```

Manual QA runs the release artifact and inspects audible output, TUI behavior,
sanitized logs, persisted files, backup restoration, D-Bus/status integration,
and shutdown. Record expected versus actual behavior and any
once-seen failure. Code reading, fake audio, or a green unit suite alone is not
a QA pass.

Verification commands:

```sh
mise install
mise run ci
mise run fuzz-deep
mise run release-build
```

`mise.toml` runs normal checks on Rust `1.97.1` and repeats the locked MSRV
subset on Rust `1.95.0`. There are no application features in the initial
release set.

Audio-device and visualizer tests should not make CI flaky. Keep most behavior testable with fake audio samples and without requiring a real sound device.

---

## Safety And Security Notes

Suzumushi is local, but local apps still have real safety risks because they parse files and mutate user data.

Threat model:

- Malformed audio files in `suzumushi/audio/`
- Malformed `.lrc` files
- Malformed image files and embedded artwork
- Malicious tag strings with terminal escape sequences
- Symlinks pointing outside the Suzumushi root
- Symlinks or targets changed between preview, backup, and write
- Two processes mutating the same root/state/backups
- Oversized or malicious shared state consumed by status commands
- Malicious or misbehaving D-Bus session peers driving the MPRIS interface (command flooding, malformed arguments, stale or forged track IDs); arguments are validated, track/playback-generation checked, and admitted nonblockingly to the bounded command channel. A full channel returns an explicit bounded busy/error response; accepted reliable controls are never displaced.
- Malicious metadata or filenames shown in search results
- Accidental destructive lyric edits
- Accidental destructive tag edits
- Accidental destructive batch tag edits
- Corrupt writes during tag editing
- Corrupt writes during lyric editing
- Unbounded scan work from huge folders
- Notification spam from rapid track changes
- Unbounded DSP, visualizer, or artwork buffers
- Supply-chain or release credential compromise and artifact substitution
- Unexpected runtime IPv4/IPv6 socket or egress behavior introduced by a dependency despite the local-only product contract
- Insecure runtime-lock placement allowing another local user to forge liveness or deny startup
- Disclosure of personal recordings, paths, lyrics, and tags through backups, journals, logs, or overly permissive app-owned directories

Root `SECURITY.md` is created when the scanner introduces the first media parser
and is extended only when a later trust boundary arrives. For each real threat it records assets,
data flows, trust boundaries, attacker/capabilities, entry points, mitigations,
owner, verification evidence, residual risk, and review triggers. It is reviewed
before adding or changing a media parser, image/tag codec, unsafe/FFI boundary,
mutation format, privilege/credential boundary, package channel, or network
capability.

Rules:

- Treat audio files, tags, filenames, and lyrics as untrusted input
- Treat images and embedded artwork as untrusted input
- Never render raw control characters from tags, lyrics, filenames, search results, tmux output, or Zellij output
- Treat notification, Waybar/Pango, tmux, and `zjstatus` formatting contexts separately from terminal-control stripping; escape or neutralize the active format language and cap output bytes
- Bound scan depth and file count
- Bound `.lrc` file size and line count
- Bound artwork source bytes, decoded dimensions, and cache size
- Bound pitch-preserving speed buffers
- Bound visualizer frame size and update frequency
- Bound search result count and avoid search work on every render
- Do not capture microphone or system audio for the visualizer
- Do not follow symlinked directories in v1
- Acquire the per-user global active-TUI lock before the canonical-root writer lock when mutable TUI work arrives; intentionally refuse another same-UID login session and allow bounded read-only status commands
- Revalidate symlink and file identity at use time, not only at scan/preview time
- Show symlink targets before tag edits
- Show lyric target paths before lyric edits
- Back up before writing tags using the verified full-file backup policy
- Refuse tag writes without a successful verified full-file backup
- Refuse lyric replacement without a successful lyric backup; creation of a verified-absent lyric path uses the durable absent-marker recovery path instead
- Journal mutation recovery durably; permit cancellation only before commit, retain the root lock during committed work regardless of normal shutdown timeout, and never detach mutation tasks
- Require explicit preview before batch tag writes
- Debounce track-change notifications
- Never block the real-time audio callback on control/event/log channels; external D-Bus calls use bounded nonblocking admission and accepted controls remain reliable
- Do not log full uncontrolled metadata if it can include terminal escapes
- Rotate logs within configured byte/count limits and omit raw lyrics, tag values, paths outside the root, and state payloads by default
- Create and verify private runtime, state, backup, journal, manifest, and log storage with the documented owner/mode contract; never broaden source access in a backup
- Read and write the single reconstructible state projection with byte/schema/version bounds, user-only permissions, no-follow checks, same-directory atomic replacement without durability sync, current boot/process/instance/lock identity, and stale-generation detection
- Do not panic on decoder, parser, scanner, visualizer, image, D-Bus, notification, Waybar, tmux, or Zellij errors
- Keep all IPv4/IPv6 network access out of v1 and verify the exact packaged release artifact under blocked-and-observed egress; local `AF_UNIX` D-Bus and audio sockets remain allowed
- Build public artifacts only from version tags, publish SHA-256 checksums, and record the source commit and `Cargo.lock` digest in the GitHub Release notes

V1 has no online features, tokens, remote API secrets, network listener, or
network-reachable service surface. It still has local filesystem/parser,
process, terminal, status-file, and D-Bus/MPRIS attack surfaces covered by the
threat register. Keep remote networking out until the local player is excellent.

---

## Resource Bounds

Important paths:

```txt
startup scan
search filtering
TUI redraw
audio playback
pitch-preserving DSP
visualizer frame generation
lyrics lookup
lyric edit writes
artwork decode/cache
Waybar state writes
tmux state writes
Zellij state writes
desktop notifications
tag backup/write
```

Initial bounds:

```txt
scan max files: 50000
scan max depth: 16
scan max playlists: 2000
entries per playlist: 10000
queue items: 10000
queue snapshots: 48 MiB
queue history items: 1000, oldest evicted at the bound; previous walks only retained history
leader chord timeout: 250 ms default, configurable via leader_timeout_ms
path bytes: 4 KiB each, 24 MiB aggregate retained capacity
metadata field: 16 KiB
total in-memory metadata: 48 MiB
process-wide app-owned allocation budget: 384 MiB
app-memory reservations: 384 MiB total: active/replacement scan indexes 112 MiB each, queue 48 MiB, artwork decode 32 MiB, parser/DSP/channels 32 MiB, mutation workspaces 16 MiB, UI/state/logging 16 MiB, unassigned headroom 16 MiB
scan/search index inclusive total: 112 MiB each; active and replacement indexes are simultaneously reserved
scan warnings: 8 MiB within the inclusive index total
lyrics max bytes: 256 KiB
lyrics max lines: 5000
lyric backup budget: 256 MiB
lyric backup entries: 10000
search max visible results: 200
search query bytes: 4 KiB
artwork source bytes: 32 MiB
artwork max decode bytes: 32 MiB
artwork max dimensions: 4096x4096
artwork disk-cache budget: 32 MiB; app-memory decode workspace: 32 MiB
artwork notification pins: 4 leases, notification timeout plus 5 seconds; MPRIS current art pinned until replacement
status event-write coalesce: 250 ms default, 50 ms compiled minimum; no progress heartbeat
status freshness/position clock: current boot ID plus suspend-aware CLOCK_BOOTTIME; Unix time diagnostic only
state file bytes: 1 MiB
status output bytes: 4 KiB
command/event/visualizer channels: 32 / 64 / 2
position channel: capacity 1, latest value
audio prefetch frames: 16384
TUI progress tick: 4-10 Hz
visualizer update rate: 8-12 Hz
visualizer max bars: 32
tag backup budget: 2 GiB
tag backup entries: 10000
journal/manifest records: 1 MiB each, 10000 entries, 64 MiB aggregate, 10000 startup recovery attempts
max batch tag edit files: 200
tag field bytes: 16 KiB
logs: 10 MiB each, 5 files, 16 KiB/record, 512 queued records, 8 MiB queued bytes, drop-and-count when full
owned descriptors/blocking jobs/parser helpers/Tokio workers: 64 / 4 / 2 / 2
owned-worker shutdown deadline: 5 seconds
committed mutation shutdown: no normal timeout; await durable completion/recovery
```

Resource-use rules:

- Do not scan on every frame
- Do not rebuild the search index on every keypress
- Do not run search matching on every render; run it on query/index changes
- Do not write shared now-playing state on every frame or maintain duplicate derived status files
- Do not read tags on every render
- Do not decode artwork on every render
- Do not block UI on file IO
- Do not allocate giant strings for lyrics or tags
- Do not keep duplicate copies of whole audio files in memory
- Do not use unbounded audio buffers for pitch-preserving speed
- Do not run visualizer sample reduction on the render path
- Do not buffer visualizer frames if the UI falls behind
- Do not add FFT visualizers in v1
- Do not send desktop notifications for progress, seek, pause, or repeated same-track events
- Use virtualized list rendering for large libraries
- Keep redraws event-driven plus low-frequency playback ticks
- Account every app-owned allocation and capacity, including active/replacement scan indexes, strings, paths, warnings, models, lookup tables, queues, parser/DSP/channel buffers, artwork, state, journals, backups, and logging against the single 384 MiB app ledger; sub-budgets never multiply that allowance.

If real libraries show that startup scanning is a problem, reconsider a cache after v1. A stale cache is worse than a simple scan for v1.

---

## Release And Packaging

V1 target:

```txt
Linux `x86_64-unknown-linux-gnu` required for v1; generic x86_64 ISA, glibc >= 2.35, Linux >= 5.15
Linux aarch64 is post-v1 until the full audio/DSP/desktop/QA matrix passes on native hardware
```

Install paths to support later:

```txt
cargo install --locked suzumushi
AUR: suzumushi-bin
GitHub Releases binary
```

`cargo install --locked suzumushi` installs only the canonical executable because Cargo
builds it from published source and does not run package hooks that create symlinks;
it is not the digest-verified prebuilt release artifact. `suzumushi-bin` and approved
installer/prebuilt flows additionally install a relative `suzu -> suzumushi`
link and test both names. Cargo users may create the same link in their own
`$CARGO_HOME/bin` or define a shell alias using documented commands; this is
optional and never represented as a second compiled binary.

These are promised channels only when their full release machinery exists.
Cargo publication, `suzumushi-bin`, and native Linux prebuilt binaries each require a named external
repository or registry integration (including the dedicated AUR packaging repo
and the crates.io/GitHub release integrations), reviewed checksum/hash updates,
a named rollback owner,
and clean-host install/upgrade/uninstall tests against the exact artifact or
source digest. Suzumushi does not claim bit-for-bit reproducibility without
separately proving it. A channel that lacks ownership, rollback, or clean-host evidence remains
documented as planned rather than advertised as available.

Versioning follows the milestone-to-version map in the Phases section: tagged
`v0.x` milestone releases during development, `v1.0.0-rc.N` candidates during
Phase 13, and `v1.0.0` as the first advertised release. Package and changelog
versions omit the `v`. The reviewed release
commit, not the later unchanged tag operation, moves the `Unreleased` section of
`CHANGELOG.md` into that version's dated section.

Document these external integrations:

```txt
Waybar custom module
tmux status output
Zellij status output through zjstatus
slash local-audio search
Space+/ global palette
playerctl usage
desktop media keys through MPRIS
MPRIS album art
desktop notifications
sidecar and shared local .lrc lyrics
local lyric editing and backups
terminal album art
terminal mini visualizer
single and batch tag editing and backups
```

Example commands:

```sh
suzumushi init ./suzumushi
suzumushi --root ./suzumushi
suzumushi waybar --root /absolute/path/to/suzumushi
suzumushi tmux --root /absolute/path/to/suzumushi
suzumushi zellij --root /absolute/path/to/suzumushi
playerctl -p suzumushi play-pause
playerctl -p suzumushi next
```

### Companion website after v1

The website is a separate post-v1 deliverable and does not block or share the
Rust application's phases, dependencies, CI, or release workflow.

- Create a separate repository named `suzumushi-front` only after Phase 13 has produced stable installation commands and a real changelog.
- Use a small static Astro site inspired by the separation used for `kickoutchi-front`, but give Suzumushi its own calm visual identity rather than cloning that design.
- Keep the initial routes focused: a landing page that explains what Suzumushi is and how its filesystem-first workflow operates, an install page showing only actually shipped channels, and a changelog page sourced from released project content.
- Keep the main `suzumushi` repository and its tagged releases authoritative. Website copy must not invent commands, availability, compatibility, or features ahead of the application.
- Deploy the generated static `dist/` through Cloudflare Pages Git integration, retain preview deployments for website changes, and attach the existing `suzumushi.org` Cloudflare domain when the production site is ready.
- Do not add Pages Functions, a database, accounts, analytics requirements, or another backend unless a later concrete website requirement needs them.

---

## Final Repo Structure

```txt
suzumushi/
|-- .gitignore
|-- .github/workflows/ci.yml
|-- .github/workflows/release.yml
|-- Cargo.toml
|-- Cargo.lock
|-- mise.toml
|-- deny.toml
|-- README.md
|-- LICENSE                          # Apache-2.0
|-- CHANGELOG.md
|-- SECURITY.md
|-- docs/
|   |-- install.md
|   |-- audio-architecture.md
|   |-- codec-compatibility.md
|   |-- testing.md
|   |-- integrations.md
|   |-- parser-isolation.md
|   |-- tag-mutation-compatibility.md
|   |-- support-matrix.md
|-- docs/adr/
|   |-- audio-pipeline.md
|-- fuzz/                              # excluded package; owns an empty [workspace]
|   |-- Cargo.toml                     # path-depends on the root library
|   |-- fuzz_targets/
|   |-- corpus/
|-- tests/
|   |-- cli.rs
|   |-- scanner.rs
|   |-- playback.rs
|   |-- mutation_safety.rs
|   |-- root_and_global_locks.rs
|   |-- desktop.rs
|-- src/
    |-- main.rs
    |-- lib.rs
    |-- cli.rs
    |-- config.rs
    |-- paths.rs
    |-- terminal.rs
    |-- app.rs
    |-- event.rs
    |-- input.rs
    |-- command.rs
    |-- errors.rs
    |-- model/
    |-- scan/
    |-- search/
    |-- audio/
    |-- dsp/
    |-- visualizer/
    |-- lyrics/
    |-- lyrics_editor/
    |-- backups/
    |-- tags/
    |-- artwork/
    |-- desktop/
    |-- status/
    |-- ui/
```

This is a target layout, not a project-setup scaffolding checklist. Create a module
only when its first behavior and tests arrive.

---

## Future Ideas After V1

Good later additions:

- Inotify-based live rescan
- ReplayGain display/support
- Lyrics offset adjustment
- Smart playlists from local tags
- Optional SQLite index only if real libraries show startup scanning is a problem
- Optional FFT spectrum visualizer
- Optional custom Zellij WASM plugin if users want richer in-session controls
- Optional `zellij pipe` integration for push-style status updates instead of polling
- Long-form resume positions and bookmarks for podcasts, audio dramas, actual-play recordings, and audiobooks

Explicitly excluded after v1 as well as in v1:

- Spotify, Apple Music, YouTube Music, and all other streaming-service integrations
- Service accounts, OAuth, cloud libraries, cloud sync, and recommendations
- Podcast/RSS subscriptions, automatic episode fetching, and download management
- Remote control over the network
