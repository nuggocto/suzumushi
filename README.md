# Suzumushi

Suzumushi is a calm, fully local terminal audio player for Linux. The current
development build creates its own filesystem root, scans local media safely,
and opens a simple keyboard-driven shell with Library, Player, and Queue panels.

Playback is the next part of the roadmap. Album artwork, terminal image
protocols, visualizers, tag editing, streaming, accounts, and network features
are deliberately out of scope.

## Run from the repository

```sh
cargo run --locked -- init ./suzumushi
cargo run --locked -- diagnose --root ./suzumushi
cargo run --locked -- --root ./suzumushi
```

Put audio below `suzumushi/audio/library/`, or copy and symlink files into
immediate child folders below `suzumushi/audio/playlists/`. In the terminal
shell, use `Tab` and `Shift+Tab` to move focus and `q` to quit.

## Install the current checkout

```sh
cargo install --path . --locked
suzumushi init ./suzumushi
suzumushi --root ./suzumushi
```

Version `0.2.0` is the released terminal foundation. Verified binary releases
and the `suzumushi-bin` AUR package arrive with the finished player. See
`PROJECT.md` for the concise roadmap.
