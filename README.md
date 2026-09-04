# Suzumushi ༼⁠ ⁠つ⁠ ⁠◕⁠‿⁠◕⁠ ⁠༽⁠つ

Suzumushi is a small, fully local audio player for Linux terminals. It reads
audio from folders, searches it, builds a queue, and plays it. There is no
account, database, streaming service, or network access.

The interface has three panels: Library, Player, and Queue. It follows the
terminal's own colors and stays out of the way. Suzu, the small bell-cricket
mascot, dances in the Player while a compact live spectrum follows the sound.
Suzu rests and the spectrum falls quiet when playback pauses or stops.

## Install on Arch Linux

Install [`suzumushi-bin`](https://aur.archlinux.org/packages/suzumushi-bin)
from AUR:

```sh
yay -S suzumushi-bin
```

The package installs both `suzumushi` and the relative `suzu` command symlink.
The verified archive below remains available for direct installation.

## Install the Linux release

The verified binary archive is built for 64-bit GNU/Linux. Download the archive
and `SHA256SUMS` from the
[`v1.1.2` release](https://github.com/nuggocto/suzumushi/releases/tag/v1.1.2),
either in your browser or with:

```sh
curl -fLO \
  'https://github.com/nuggocto/suzumushi/releases/download/v1.1.2/{SHA256SUMS,suzumushi-v1.1.2-x86_64-unknown-linux-gnu.tar.xz}'
```

Then verify and install the downloaded archive:

```sh
sha256sum --check --ignore-missing SHA256SUMS
tar -xf suzumushi-v1.1.2-x86_64-unknown-linux-gnu.tar.xz
cd suzumushi-v1.1.2-x86_64-unknown-linux-gnu
sudo install -Dm755 suzumushi /usr/local/bin/suzumushi
sudo ln -s suzumushi /usr/local/bin/suzu
```

The checksum must report `OK` before installation. The `suzu` command is only a
relative packaging symlink; `suzumushi` remains the one canonical executable.

## Install the current checkout

You need Rust 1.95 or newer plus the ALSA, PipeWire, D-Bus, and Clang
development files:

- Arch Linux: `alsa-lib dbus pipewire clang pkgconf`
- Debian or Ubuntu: `libasound2-dev libpipewire-0.3-dev libspa-0.2-dev libdbus-1-dev libclang-dev pkg-config`
- Fedora: `alsa-lib-devel pipewire-devel dbus-devel libclang-devel pkgconf-pkg-config`

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
│   └── D&D/
└── playlists/
    ├── Evening/
    └── Favorites/
```

Every immediate folder below `audio/playlists/` is a playlist. It may contain
copied audio files or file symlinks. Suzumushi never scans `~/Music`
automatically.

Press `Enter` on a Library folder to collapse or expand it. This only changes
the current terminal view: search still finds tracks inside collapsed folders,
and the Queue and playback are untouched. Folder visibility resets when
Suzumushi closes.

The last non-empty Queue, current track, and playback position are restored
when the player reopens. Playback remains paused until you press `Space`, or
press `Enter` while Player or Queue is focused.

## Keys

| Key | Action |
| --- | --- |
| `Tab` or `Shift+Tab` | Move between Library, Player, and Queue |
| Up or Down, or `j` or `k` | Move the selection |
| `Home` or `End`, or `g` or `G` | Jump to the first or last item |
| `Enter` | In Library, toggle a folder or add a track or playlist; in Player or Queue, play or pause |
| `/` | Search artist, title, filename, and relative path |
| `Esc` | Close search or the built-in key guide |
| `J` or `K` | In Queue, move the selected item down or up |
| `d` or `Delete` | In Queue, remove the selected item |
| `c` | In Queue, clear every item |
| `Space` | Play or pause |
| `s` | Stop |
| `p` | Play the previous track |
| `n` | Play the next track |
| Left or Right | Seek backward or forward five seconds; hold to move quickly |
| `-` or `+` | Lower or raise volume by five percent |
| `m` | Mute |
| `x` | Toggle shuffle |
| `r` | Cycle repeat off, queue, and one |
| `?` | Open or close the built-in key guide |
| `q` | Quit outside search |
| `Ctrl+c` | Quit from anywhere |

Search checks metadata, filenames, and paths. Pressing `Enter` queues the
selected result. Pressing `Enter` on a playlist adds all of its tracks as one
Queue change. If nothing is playing, it starts immediately. An underlying audio
file can appear in the Queue only once. Selecting it again, directly or through
a playlist, leaves the Queue unchanged and reports `Audio already in Queue`.
Playlist additions remain all-or-nothing. Shuffle keeps the editable Queue in
its visible order while randomizing playback across its tracks.

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
`NO_COLOR`, or use `theme = "mono"`, for a monochrome interface. The focused
panel is marked with `>` and a bold single-line border, so focus never depends
on color alone.

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

Version `1.1.2` is the current stable release. Its tag-driven Linux archive is
fully verified before GitHub publishes it. Arch users can install the matching
`suzumushi-bin` package from AUR.

Release and AUR maintenance details live in
[docs/release.md](docs/release.md).
