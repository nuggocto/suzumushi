# Suzumushi

Suzumushi is a small, fully local audio player for Linux terminals. It reads
audio from folders, searches it, builds a queue, and plays it. There is no
account, database, streaming service, or network access.

The interface has three panels: Library, Player, and Queue. It follows the
terminal's own colors and stays out of the way. Suzu, the small bell-cricket
mascot, dances in the Player while audio is playing and rests when it is not.

## Install the current checkout

You need Rust 1.95 or newer and the ALSA development files:

- Arch Linux: `alsa-lib`
- Debian or Ubuntu: `libasound2-dev`
- Fedora: `alsa-lib-devel`

Install and create a local Suzumushi root:

```sh
cargo install --path . --locked
suzumushi init ./suzumushi
```

Add audio below `suzumushi/audio/library/`, then start the player:

```sh
suzumushi --root ./suzumushi
```

To inspect the root without opening the player:

```sh
suzumushi diagnose --root ./suzumushi
```

## Organize the library

The filesystem is the library. Use any folder structure you want:

```text
suzumushi/audio/
├── library/
│   ├── Music/
│   ├── Audiobooks/
│   └── JDR/
└── playlists/
    ├── Evening/
    └── Favorites/
```

Every immediate folder below `audio/playlists/` is a playlist. It may contain
copied audio files or file symlinks. Suzumushi never scans `~/Music`
automatically.

The last non-empty Queue, current track, and playback position are restored
when the player reopens. Playback remains paused until you press `Space`.

## Keys

| Key | Action |
| --- | --- |
| `Tab` or `Shift+Tab` | Move between Library, Player, and Queue |
| Up or Down, or `j` or `k` | Move the selection |
| `Home` or `End`, or `g` or `G` | Jump to the first or last item |
| `Enter` | Add the selected Library item to the Queue and start it when idle |
| `/` | Search artist, title, filename, and relative path |
| `Esc` | Close search |
| `J` or `K` | In Queue, move the selected item down or up |
| `d` or `Delete` | In Queue, remove the selected item |
| `c` | In Queue, clear every item |
| `Space` | Play or pause |
| `s` | Stop |
| `n` or `p` | Play the next or previous track |
| Left or Right | Seek backward or forward five seconds; hold to move quickly |
| `-` or `+` | Lower or raise volume by five percent |
| `m` | Mute |
| `x` | Toggle shuffle |
| `r` | Cycle repeat off, queue, and one |
| `?` | Open or close the built-in key guide |
| `q` | Quit outside search |
| `Ctrl+c` | Quit from anywhere |

Search checks metadata, filenames, and paths. Pressing `Enter` queues the
selected result. If nothing is playing, it starts immediately.

Held seek input keeps only the newest target, so releasing the arrow key resumes
from the position shown instead of replaying obsolete intermediate seeks.

## Desktop controls

While the terminal player is open, Suzumushi exposes one standard MPRIS player.
Desktop media keys and tools such as `playerctl` use the same play, pause, stop,
next, previous, seek, volume, shuffle, and repeat actions as the terminal. For
example:

```sh
playerctl --player=suzumushi play-pause
playerctl --player=suzumushi next
playerctl --player=suzumushi metadata
```

Only bounded text metadata is published. Suzumushi does not publish artwork or
accept files and URLs through MPRIS.

## Terminal colors

Suzumushi inherits the terminal's foreground, background, and named ANSI
palette. Changing the terminal theme changes the player with it. Set
`NO_COLOR`, or use `theme = "mono"`, for a monochrome interface.

## Audio support

The verified formats are:

- MP3 with MPEG Layer III
- FLAC
- WAV with signed 16-bit PCM
- Ogg with Vorbis

Mono and stereo audio from 8 kHz through 192 kHz is accepted when the default
audio device supports the source rate. AAC, M4A, Opus, multichannel audio, and
automatic resampling are not current support claims.

## Status

Version `0.4.0` is the released complete local player with MPRIS, global media
keys, finished terminal states, built-in help, and the verified local
installation path. The remaining work is the verified Linux release and
`suzumushi-bin` AUR package.

The full contract and roadmap live in [PROJECT.md](PROJECT.md).
