# Suzumushi ༼⁠ ⁠つ⁠ ⁠◕⁠‿⁠◕⁠ ⁠༽⁠つ

Suzumushi is a small, fully local audio player for Linux terminals. It reads
audio from folders, searches it, builds a queue, and plays it. There is no
account, database, streaming service, or network access.

The interface has three panels: Library, Player, and Queue. They use the
terminal's own colors and never paint a background. In the Player, Suzu, the
small bell-cricket mascot, dances in a night meadow. Each frequency band is a
grass stalk tipped with light that rides the music. When playback pauses, the
lights lift off as fireflies and drift around Suzu until you resume. When it
stops, the meadow settles into quiet, dewy grass. In a tall Player, a night sky
opens above the title, with faint twinkling stars and a crescent moon that
crosses it as the track plays.

## Install on x86-64 Linux

Run:

```sh
curl -fsS https://suzumushi.org/install.sh | sh
```

The installer puts `suzumushi` and the `suzu` shortcut in `~/.local/bin`
without sudo. It checks the release archive against the SHA-256 recorded in the
script and confirms the player starts before replacing anything. The player
needs the ALSA, PipeWire, and D-Bus runtime libraries, which most desktop
distributions already include. Pass a different directory with
`sh -s -- --bin-dir /absolute/directory`.

## Install on Arch Linux

Install [`suzumushi-bin`](https://aur.archlinux.org/packages/suzumushi-bin)
from AUR:

```sh
yay -S suzumushi-bin
```

The package installs both `suzumushi` and the relative `suzu` command symlink.

## Install on NixOS

On x86-64 NixOS with Nix installed, add the repository flake to your user profile:

```sh
nix --extra-experimental-features 'nix-command flakes' \
  profile add github:nuggocto/suzumushi/v1.4.3
```

This installs both `suzumushi` and `suzu`. Nix builds from source with pinned
dependencies; the first installation may take a few minutes. Version 1.1.4 is
the first stable release containing the flake.

Create your library, add audio below `./suzumushi/audio/library/`, and start:

```sh
suzumushi init ./suzumushi
suzumushi --root ./suzumushi
```

Your desktop needs a working audio session. See [Nix packaging](docs/nix.md) for
declarative NixOS and Home Manager installation, running without installation,
and local build checks.

## Install the release archive by hand

To check each step yourself, download the archive and `SHA256SUMS` from the
[`v1.4.3` release](https://github.com/nuggocto/suzumushi/releases/tag/v1.4.3):

```sh
curl -fLO \
  'https://github.com/nuggocto/suzumushi/releases/download/v1.4.3/{SHA256SUMS,suzumushi-v1.4.3-x86_64-unknown-linux-gnu.tar.xz}'
```

Then verify and install the downloaded archive:

```sh
sha256sum --check --ignore-missing SHA256SUMS
tar -xf suzumushi-v1.4.3-x86_64-unknown-linux-gnu.tar.xz
cd suzumushi-v1.4.3-x86_64-unknown-linux-gnu
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

Unchanged files reuse tags that fit in `state/metadata.jsonl`. The cache holds
up to 50,000 files and 16 MiB of tag text. The size of the encoded JSON can
limit it further. Suzumushi replaces the file only when its contents change.
Delete it while the player is closed to force a fresh read. `diagnose` reads the
cache but never writes it.

On a large library, the scan shows a running track count and spends up to
about a minute reading uncached tags. Tracks it doesn't reach show their
filenames, with a warning. Later starts reuse cached tags and try the rest.

Press `Enter` on a Library folder to collapse or expand it. This changes only
the current view. Search still finds tracks inside collapsed folders, the Queue
and playback stay as they are, and every folder opens again the next time
Suzumushi starts.

When the player reopens, it restores the Queue, the current track, and the
playback position. Playback stays paused until you press `Space`, or `Enter`
while Player or Queue is focused. Clearing the Queue also forgets the saved
session. If a saved session can't be restored exactly, because some of its
tracks are missing or the file comes from an unsupported version, Suzumushi
keeps `state/session.json` as it is until you edit the Queue.

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

`SIGTERM` and `SIGINT` quit too. Once the player is running, they restore the
terminal and save the session as `q` does. Closing the terminal window ends the
player at once. The next launch uses the last saved Queue, track, and position.
During playback, the player saves a moving position about every five seconds.
A recent seek can leave a larger gap between the saved and current positions.

Closing the window works through the terminal's hangup signal. A player started
without the window as its controlling terminal, for example under `setsid`,
never receives it, so quit that player with `q` before you close the window.

Search checks metadata, filenames, and paths. Pressing `Enter` queues the
selected result. Pressing `Enter` on a playlist adds all of its tracks as one
Queue change. If nothing is playing, it starts immediately. An underlying audio
file can appear in the Queue only once. Selecting it again, directly or through
a playlist, leaves the Queue unchanged and reports `Audio already in Queue`.
Playlist additions remain all-or-nothing. Shuffle keeps the editable Queue in
its visible order while randomizing playback across its tracks.

Held seek input keeps only the newest target, so releasing the arrow key resumes
from the position shown instead of replaying obsolete intermediate seeks. Audio
stays quiet while the key is held, and pausing, resuming, and seeking fade in
and out instead of cutting abruptly.

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

## Configuration

`suzumushi init` writes `config.toml` in the root, with a comment above each
setting:

```toml
theme = "terminal"        # or "mono"
background = "auto"       # or "dark" or "light"
default_view = "library"  # or "queue"

[scan]
cross_mounts = false
follow_file_symlinks = true
ignore_hidden_audio = true
```

Every setting is optional. Config files from 1.2 and earlier keep working.
Their capacity limits are compiled in now, so Suzumushi ignores those keys. Each
root admits one player at a time; players on different roots can run together.

The session log is `logs/suzumushi.log`, one timestamped line per record. It
keeps one 1 MiB archive.

## Terminal colors

Suzumushi uses the terminal's foreground, background, and named ANSI colors, so
changing the terminal theme changes the player too. It never paints a
background.

The Player's meadow, its sky, and the progress line are the exception. They draw
their own colors, with a gradient from peach for bass through rose and lilac to
mint for treble. They use 24-bit color when the terminal sets
`COLORTERM=truecolor`, and the nearest of 256 colors otherwise.

Those colors come in two sets. A dark terminal gets pastels. A light terminal
gets deeper shades of the same hues, because pastels disappear on a light
background. At startup Suzumushi asks the terminal for its background color. If
the terminal doesn't answer, Suzumushi reads `COLORFGBG`, and if that isn't set
either, it uses the dark set. To skip the question, set `background = "light"`
or `background = "dark"` in `config.toml`. The log names the choice, for
example `player colors for a light background (#fdf6e3)`.

Set `NO_COLOR`, or use `theme = "mono"`, for a monochrome interface. The focused
panel shows `>` before its title and a bold border, so focus never depends on
color.

## Audio support

The verified formats are:

- MP3 with MPEG Layer III
- FLAC
- WAV with signed 16-bit PCM
- Ogg with Vorbis

Suzumushi plays mono and stereo audio from 8 kHz through 192 kHz. It doesn't
resample, so the default audio device or server has to accept the track's
sample rate. PipeWire resamples, so it accepts all of these rates. AAC, M4A,
Opus, and multichannel audio are not supported.

## Status

Version `1.4.3` is the current stable release. The release workflow builds and
verifies its Linux archive from the tag before GitHub publishes it. Arch users
can install the matching `suzumushi-bin` package from AUR.

Release and AUR maintenance details live in
[docs/release.md](docs/release.md).
For state ownership, module boundaries, and verification commands, see the
[code map](docs/architecture.md).
