# Code map

Suzumushi scans once, then the terminal event loop owns application state. The
CLI and desktop media controls feed the same app actions.

## App state

- `src/app.rs` coordinates actions, notices, presentation, and session snapshots.
- `src/app/browser.rs` owns browser rows, folder visibility, selection, and
  search. Search retains a bounded prefix table to avoid quadratic matching on
  repetitive input. Closing search restores the normal browser selection.
- `src/app/queue.rs` owns queue items, asset membership, item identities, and
  shuffle order. Insertion checks the complete batch before committing it;
  duplicate audio rejects the entire playlist. Queue edits advance one
  generation, and shuffle edits preserve the played prefix.
- `src/app/playback.rs` owns playback status, generations, timing, and playback
  transitions. Selecting or loading a track resets its timing together. The
  app chooses the next track; the audio worker only reports completion.

The playing item can outlive its queue membership. Removing it or clearing the
queue leaves its audio running. Its position hint keeps subsequent navigation
predictable until another track is selected.

The scan index is immutable during a session. Queue membership uses one byte
per canonical asset, while queue order and shuffle order use bounded vectors.
The app validates their startup reservations against the configured budgets.

## Audio and files

`src/terminal.rs` dispatches playback intents after verifying the selected file
against the pinned root and scanned identity. `src/audio/mod.rs` owns the worker,
commands, decoder lifecycle, and output lifecycle. Decoder helpers live in
`src/audio/decoder.rs`; their messages, buffers, and execution are bounded.

`src/audio/output.rs` converts channels into a fixed 1 MiB PCM ring. The producer
publishes complete frames, and the callback consumes complete frames or emits
silence. The callback does no allocation, locking, waiting, or logging. Ring
consumption precedes hardware playback, so completion also waits for the
backend's playback delay.

## Verification

Run `mise run ci` for formatting, Clippy, tests, MSRV compatibility, dependency
checks, and bounded fuzzing. App behavior tests live in `src/app/tests.rs`;
`tests/cli.rs` covers terminal sessions and public commands. Release and package
checks are documented in [release.md](release.md) and [nix.md](nix.md).
