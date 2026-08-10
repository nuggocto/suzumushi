# Suzumushi

Suzumushi is a calm, fully local terminal audio player for Linux. The current
development build creates its own filesystem root, scans local media safely,
browses nested library folders and folder playlists, searches local metadata,
and builds an ordered queue from the terminal.

Playback is the next part of the roadmap. Album artwork, terminal image
protocols, visualizers, tag editing, streaming, accounts, and network features
are deliberately out of scope.

## Run from the repository

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
- Arrow keys or `j`/`k`: move selection. `Home`/`End` or `g`/`G` jump to an edge.
- `Enter`: add the selected Library track, playlist, or search result to the
  Queue.
- `/`: search metadata, filenames, and relative paths. `Enter` accepts and `Esc`
  closes search. While search is open, `q` is query text.
- In Queue, `J`/`K` reorder, `d` removes, and `c` clears.
- `q`: quit outside search. `Ctrl+c`: quit from any mode.

## Install the current checkout

```sh
cargo install --path . --locked
suzumushi init ./suzumushi
suzumushi --root ./suzumushi
```

Version `0.2.0` is the released terminal foundation. Verified binary releases
and the `suzumushi-bin` AUR package arrive with the finished player. See
`PROJECT.md` for the concise roadmap.
