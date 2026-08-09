# Suzumushi

Suzumushi is a calm, fully local terminal audio player for Linux. The current
build can create its app-owned filesystem root, diagnose the local media it
finds there, and open the keyboard-driven terminal shell.

```sh
cargo run --locked -- init ./suzumushi
cargo run --locked -- diagnose --root ./suzumushi
cargo run --locked -- --root ./suzumushi
```

Put audio below `suzumushi/audio/library/`, or copy and symlink files into
immediate child folders below `suzumushi/audio/playlists/`. In the terminal
shell, use `Tab` and `Shift+Tab` to move focus and `q` to quit. Playback is not
implemented yet.
