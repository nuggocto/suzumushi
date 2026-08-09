# Terminal Runtime

The terminal shell is deliberately synchronous. `ratatui 0.30.2` renders through
`crossterm 0.29.0`; no async runtime is present because this milestone has no
background application worker. `AppState` remains owned by the single event
loop.

The direct Crossterm dependency disables defaults and asks only for events.
Ratatui's Crossterm adapter currently unifies Crossterm's default features back
into the resolved graph, so the committed lockfile and dependency policy record
that actual graph rather than claiming a smaller one.

## Startup and ownership

Startup uses this order:

1. Discover and validate the selected root and configuration.
2. Reserve the terminal session's exact five-descriptor startup peak against
   `runtime.max_open_files`; reject the session before opening anything if the
   limit is lower.
3. Verify the current UID's private `XDG_RUNTIME_DIR` and acquire the active-TUI
   lease.
4. Acquire the selected root's writer lease.
5. Verify private log storage and install bounded file-only logging.
6. Check the initial terminal cell-buffer allocation against the 8 MiB UI/state
   reservation, enable raw mode, enter the alternate screen, and hide the cursor.
7. Probe terminal image capability with one outstanding device-attributes query
   and a 150 ms and 4 KiB reply bound.
8. Run the app-owned draw and input loop.

The capability probe records `disabled`, `kitty`, `sixel`, `iterm2`, or
`fallback`. Kitty-family and iTerm2 terminals are selected from their terminal
identity. Other terminals receive one device-attributes query for Sixel, and
only a complete reply ending in `c` is accepted. A partial reply at the deadline
is an error so its remainder cannot become keyboard input. A missing reply and
pure Wayland session reach the bounded fallback. tmux and Zellij use immediate
fallback because passthrough is not part of the current shell. Artwork rendering
is not implemented. The two middle panels are still placeholders: the planned
Now Playing panel is a compact title and creator strip, while the persistent Art
panel owns album or series details, playback state, progress, elapsed/duration,
volume and speed summaries, the mini visualizer, and an optional cover region.
Toggling or falling back from cover artwork will not hide the playback details.

The alternate screen is cleared directly. Ratatui's cursor-preserving clear is
not used because it issues an unbounded cursor-position query on a terminal that
may not answer.

## Input and redraw

`q` and Ctrl-C quit. `Tab` and `Shift+Tab` cycle visible focus in spatial order:
Library, Now Playing, Art, then Queue. Panel titles remain centered and stationary
while double borders and text emphasis show focus. `Space` waits for the configured
leader timeout: `/` before the deadline selects its placeholder chord action,
while expiry fires the placeholder play/pause action.
Chord keys must be unmodified. An invalid second key either performs its one
normal action or lets the pending play/pause fire, never both.
The event poll never waits past the next leader deadline or 100 ms redraw tick.
Resize events redraw from current terminal dimensions. Terminals below 80x24
show only the bounded resize-and-quit message. Ratatui uses a fixed viewport so
it cannot allocate through hidden autoresize. Each resize checks the bytes for
both cell buffers against the 8 MiB reservation, drops the old buffers, and only
then builds the replacement. An oversized resize fails closed and the terminal
guard restores the shell before the diagnostic appears.

## Logging

Logs stay under `logs/` in current-user-owned `0600` regular files. The formatter
caps each record before it enters the lossy count-bounded queue. Configuration
also reserves the queue's worst-case bytes. One app-owned worker drains that
queue into the rotating writer, which removes archives above a lowered retention
setting and retains at most the configured file count. It refuses
symlinks, special files, hard links, foreign ownership, and broad modes. The
terminal surface is never a logging sink. Shutdown closes queue admission,
drains pending records, and joins the worker without a timeout or detached
thread. It then reports queue drops directly after terminal restoration.
Background write, flush, rotation, or worker failures make an otherwise
successful command fail.

## Restoration and crash recovery

Normal return, errors, and unwinding panics drop a guard that shows the cursor,
leaves the alternate screen, and disables raw mode. `abort`, `SIGKILL`, and power
loss cannot run Rust destructors. After such an exit, the kernel releases both
flocks automatically and the next process safely reuses their files. If the
terminal itself remains confused, run `reset`; if needed, run `stty sane` from
the affected shell.
