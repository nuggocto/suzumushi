# Code map

Suzumushi scans once, then the terminal event loop owns application state. The
CLI and desktop media controls feed the same app actions.

## App state

- `src/app.rs` coordinates actions, notices, presentation, and session snapshots.
- `src/app/browser.rs` owns browser rows, folder visibility, selection, and
  search. Search compares each field with the query, ignoring ASCII case, and
  stops at the result limit. Closing search restores the normal browser selection.
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

The scan index is immutable during a session. Queue membership uses one flag
per canonical asset, and the queue holds at most 10,000 items.

`src/config.rs` parses `config.toml`, which holds only the theme, the terminal
background, the first focused view, and three scan behaviors. Capacity limits
are compiled into `Config::default()`; tests lower them through its public
fields.

`src/state.rs` writes `state/session.json`, the resume checkpoint. The event loop
saves queue or current track changes on its next turn. It saves a changed
position once five seconds have elapsed since the last write, and saves any
remaining changes on orderly exit. When the startup restore is
inconclusive, because the file is unsupported or malformed, no saved track
matches, or some entries were skipped, the existing `session.json` stays as it
is through startup, playback, and shutdown until the user edits the queue.

## Player stage

`src/ui/stage.rs` owns the Player's night meadow, the only UI state kept between
frames. Each spectrum band is a stalk whose light is thrown up by the music and
falls back under gravity; on pause the lights fly as fireflies along
deterministic paths. On resume each light keeps its flight while an eased pull
brings it home, in a wave, and its stalk grows to meet it, so nothing stops or
jumps. Shapes render into a fixed dot field, allocated once per session, then
into braille cells: solid shapes stay solid, glow becomes an ordered stipple,
and bare stalk columns become hairlines. When the Player has room, the stage
extends above the title into a sky of fixed-share stars and a moon placed by
track progress. The title and Suzu are drawn last with silhouette masks.
`src/ui/palette.rs` holds the night and day color sets and the glow shading that
the meadow and the progress rail share. A light's brightness is its distance
from the background: by night it fades toward black, by day into the terminal's
own background color. `src/background.rs` finds that color once, in raw mode
before the first key is read. It sends an OSC 11 query followed by a DA1 query,
and the DA1 reply ends the wait, so a terminal that ignores OSC 11 costs one
round trip rather than the 300 ms timeout. `src/ui/rail.rs` draws the rail's
trail, dew, and light in the same braille dots. The stage asks the event loop
for about 60 frames per second while lights lift off or fly home, so they move
about a dot per frame, 30 while the meadow moves, 20 while fireflies drift, and
the ordinary tick once still.

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

`src/logging.rs` appends timestamped lines to `logs/suzumushi.log` under a mutex.
At 1 MiB the file becomes `suzumushi.log.1`, replacing the previous archive.

Release builds abort on panic. The terminal session installs a panic hook that
restores the terminal before the message prints, and each helper process
installs one that exits quietly, so a malformed file never produces a crash
report.

## Signals and shutdown

`SIGTERM` and `SIGINT` set a flag that the running event loop checks on every
turn. The player restores the terminal, stops audio, and writes the final
checkpoint as it does on `q`. Desktop controls shut down last,
so a slow session bus cannot hold up the checkpoint. A second `SIGTERM` or
`SIGINT`, or one that arrives once teardown has started, gets the default
action.

`SIGHUP` keeps its default action. When the terminal sends it on close, the
process exits immediately and leaves the last checkpoint on disk. A seek can
move the playhead by more than five seconds between writes. The kernel
releases the root lock, and the helpers die with their parent.
Handling `SIGHUP` would put the loop back in crossterm 0.29's input reader,
which retries a hung-up terminal forever (crossterm-rs/crossterm#1067).

A player whose pty is not its controlling terminal receives no `SIGHUP` when
the window closes and can still get stuck in that reader.
One `SIGTERM` then only sets the flag, which the stuck loop never reads, so it
takes a second one.

At system shutdown, systemd may stop the terminal emulator while the player is
still drawing or restoring the terminal. A write that fails with `EIO` or
`ENXIO` puts the output in discard mode, including cursor restoration. If the
terminal closes during setup and no `SIGHUP` arrives, a failed size query can
still end startup with an error.

## Verification

Run `mise run ci` for formatting, Clippy, tests, MSRV compatibility, dependency
checks, and bounded fuzzing. App behavior tests live in `src/app/tests.rs`;
`tests/cli.rs` covers terminal sessions and public commands. Release and package
checks are documented in [release.md](release.md) and [nix.md](nix.md).
