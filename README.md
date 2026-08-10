# Suzumushi

Suzumushi is a calm, fully local terminal audio player for Linux. The current
development build creates its own filesystem root, scans local media safely,
browses nested library folders and folder playlists, searches local metadata,
builds an ordered queue, and plays it through one local Linux audio pipeline.

Play, pause, stop, next, previous, seek, volume, mute, shuffle, repeat, and
progress are implemented. Playback speed is pitch-preserving from `0.5x`
through `2.0x`. Album artwork, terminal image protocols, visualizers, tag
editing, streaming, accounts, and network features are deliberately out of
scope.

## Run from the repository

Source builds need Rust and the ALSA development files: `alsa-lib` on Arch,
`libasound2-dev` on Debian or Ubuntu, or `alsa-lib-devel` on Fedora.

```sh
cargo run --locked -- init ./suzumushi
cargo run --locked -- diagnose --root ./suzumushi
cargo run --locked -- --root ./suzumushi
```

Put audio in any folder structure below `suzumushi/audio/library/`. Immediate
child folders below `suzumushi/audio/playlists/` are playlists and may contain
copied files or file symlinks.

The current terminal keys are:

- `Tab` and `Shift+Tab`: move panel focus.
- Up/Down or `j`/`k`: move selection. `Home`/`End` or `g`/`G` jump to an edge.
- `Enter`: add the selected Library track, playlist, or search result to the
  Queue. If playback is idle, start the new track or the playlist's first track.
- `/`: search metadata, filenames, and relative paths. `Enter` accepts and `Esc`
  closes search. While search is open, `q` is query text.
- In Queue, `J`/`K` reorder, `d` removes, and `c` clears.
- `Space`: play or pause. `s`: stop. `n`: next. `p`: previous.
- Left/Right: seek backward/forward 5 seconds. `-`/`+`: change volume by 5%.
- `m`: mute. `x`: shuffle. `r`: cycle repeat off/all/one.
- `[`/`]`: change speed by `0.25x`. `0`: restore `1.0x`.
- `q`: quit outside search. `Ctrl+c`: quit from any mode.

The interface inherits the terminal's foreground, background, and named ANSI
palette, so changing the terminal theme changes Suzumushi with it. `NO_COLOR`
or `theme = "mono"` keeps the interface monochrome.

The private `state/now-playing.json` projection is bounded and refreshed at
least every five seconds while the TUI is alive, including while paused. A
clean or failed terminal shutdown writes a final stopped state when storage is
still available.

The verified first compatibility set is MP3/MPEG Layer III, FLAC, WAV with
signed 16-bit PCM, and Ogg/Vorbis. Tests cover 48 kHz stereo through Symphonia
0.6.0, and real output is handled by CPAL 0.18.1. Mono and stereo sources from
8 kHz through 192 kHz are accepted when the default output device supports the
source rate. AAC, M4A, Opus, multichannel audio, and implicit resampling are not
current support claims.

## Install the current checkout

```sh
cargo install --path . --locked
suzumushi init ./suzumushi
suzumushi --root ./suzumushi
```

Version `0.2.0` is the released terminal foundation. Verified binary releases
and the `suzumushi-bin` AUR package arrive with the finished player. See
`PROJECT.md` for the concise roadmap.
