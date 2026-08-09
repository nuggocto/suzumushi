# Suzumushi

Suzumushi is a calm, fully local terminal audio player for Linux. The current
build can create its app-owned filesystem root and diagnose the local media it
finds there.

```sh
cargo run --locked -- init ./suzumushi
cargo run --locked -- diagnose --root ./suzumushi
```

Put audio below `suzumushi/audio/library/`, or copy and symlink files into
immediate child folders below `suzumushi/audio/playlists/`. Shared `.lrc` files
belong in `suzumushi/audio/lyrics/`. Run the bare command or `--help` to see the
available command-line surface.
