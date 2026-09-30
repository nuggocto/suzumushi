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

## Player stage

`src/ui/stage.rs` owns the Player's night meadow, the only UI state kept between
frames. Each spectrum band is a stalk whose light is thrown up by the music and
falls back under gravity; on pause the lights fly as fireflies along
deterministic paths. On resume each light keeps its flight while an eased pull
brings it home, in a wave, and its stalk grows to meet it, so nothing stops or
jumps. Shapes render into a fixed
dot field, reserved once and counted in the UI budget, then into braille cells:
solid shapes stay solid, glow becomes an ordered stipple, and bare stalk columns
become hairlines. When the Player has room, the stage extends above the title
into a sky of fixed-share stars and a moon placed by track progress. The title
and Suzu are drawn last with silhouette masks. `src/ui/palette.rs` holds the
pastel gradient and glow shading that the meadow and the progress rail share;
`src/ui/rail.rs` draws the rail's trail, dew, and light in the same braille dots. The stage asks the
event loop for about 30 frames per second while it moves, 20 while fireflies
drift, and the ordinary tick once still.

The audio callback analyzes 32 bands from 50 Hz to 16 kHz and normalizes them
against a slowly relaxing loudness reference, so loud masters and quiet
recordings use the same visual range.

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

Output never cuts hard. The callback ramps gain over 8 ms whenever output
starts, pauses, resumes, mutes, or stops, and a toggle mid-ramp reverses it. A
pause leaves the backend stream running and silent until it has been paused for
half a second, so rapid toggles never restart the device stream mid-cycle.
Teardown waits, briefly, for the fade-out to reach the backend. Seeks arriving
within 150 ms of a restart are held silently and applied once, at the latest
target, so a held arrow key restarts the decoder once.

## Verification

Run `mise run ci` for formatting, Clippy, tests, MSRV compatibility, dependency
checks, and bounded fuzzing. App behavior tests live in `src/app/tests.rs`;
`tests/cli.rs` covers terminal sessions and public commands. Release and package
checks are documented in [release.md](release.md) and [nix.md](nix.md).
