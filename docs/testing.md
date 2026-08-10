# Executable QA Record

The cases here cover the currently runnable root, scanner, and terminal-shell
surface. Each run uses a fresh local temporary directory and removes it from the
workspace after evidence is checked.

Last verdict: **PASS** on 2026-08-10 (Europe/Paris), from the working tree based
on commit `1259e59d12c7` on branch `shrek`. `mise run ci` passed from this tree with
Rust 1.97.1, the Rust 1.95.0 MSRV, dependency policy checks, 63 automated tests,
and all five bounded fuzz smoke targets. The tested debug executable SHA-256 was
`9663a241d47300da257eb0e97e5bd63535397924e03840d84987fbb25cf1a72b`.
The terminal flow uses that real executable attached to an 80x24 pseudo-terminal;
test-only roots, runtime storage, logs, and terminal descriptors are removed
when the case ends.

## Automated test layout

Pure module behavior stays in `#[cfg(test)]` modules beside its implementation.
The root `tests/` directory contains only crate-boundary tests: executable flows,
filesystem behavior, kernel locking, root discovery, and scanner behavior over
real temporary directories. Untrusted parser and projection inputs remain in
the dedicated `fuzz/` targets.

| Case | User flow and oracle | Evidence status |
|---|---|---|
| `SUZU-QA-ROOT-001` | Run `suzumushi init <fresh-root>`; inspect the full tree, instructions, `0700` private directories, and `0600` config | PASS |
| `SUZU-QA-SCAN-001` | Add copied and symlinked audio, artwork, malformed and broken entries; run `diagnose --root`; require bounded counts, metadata/fallback output, and warnings without panic | PASS |
| `SUZU-QA-SCAN-002` | Add `audio/.git/index`, lower `max_entries` to the visible-tree count plus the hidden directory entry, and run `diagnose --root`; require `complete: true` with no descendant budget charge or warning | PASS |
| `SUZU-QA-SCAN-003` | Add `audio/notes/ignored.mp3` beside the owned media hierarchy; run `diagnose --root`; require the unrelated subtree to be ignored without consuming descendant budgets | PASS |
| `SUZU-QA-CLI-001` | Run incomplete `suzumushi init`; require exit 2, empty stdout, and one clean `error:` prefix with no escaped newline | PASS |
| `SUZU-QA-FS-001` | Exercise external, broken, magic, directory, and non-regular symlink targets; require verified regular descriptors or explicit bounded refusal | PASS (CLI smoke plus automated race/magic-link coverage) |
| `SUZU-QA-FIFO-001` | Put FIFOs at a media target, `config.toml`, and an expected README path; require media diagnostics to warn, config/init to refuse clearly, and every command to return within the two-second test bound | PASS |
| `SUZU-QA-INIT-002` | Replace a nested expected directory with a file; require repeated initialization to report the exact nested path | PASS (automated) |
| `SUZU-QA-INIT-003` | Remove an owned entry, hold the root writer lease, and rerun initialization; require clear contention with no repair, then verify repair only after release and refuse an unsafe lock mode | PASS (automated) |
| `SUZU-QA-ORDER-001` | Diagnose nested `library/a/x.mp3` and `library/a-/x.mp3`; require component-wise order with `a` before `a-` | PASS |
| `SUZU-QA-PATH-001` | Diagnose one invalid-byte filename and one literal `\\xFF` filename; require distinct terminal-safe output with no raw control bytes | PASS |
| `SUZU-QA-MEM-001` | Reserve the old and replacement indexes plus scanner/parser scratch; accept the reviewed partition and reject one byte above it before traversal | PASS (automated) |
| `SUZU-QA-MEM-002` | Deliver a real PTY resize whose two Ratatui cell buffers exceed the 8 MiB UI reservation; require refusal before growth, terminal restoration, and lease release | PASS |
| `SUZU-QA-LOCK-001` | Hold root and per-user leases; require second owners to fail while bounded readers continue, then verify release and stale-file reuse | PASS (automated; diagnostics confirmed lock-free) |
| `SUZU-QA-TUI-001` | Start the real executable on an 80x24 pseudo-terminal with no image-capability reply; require alternate-screen entry and visible Library panel, send `Tab` and observe the rendered Now Playing focus, deliver `SIGWINCH` at 40x10 and observe the fallback, then require `q` exit, cursor/alternate-screen restoration, and no file-log text on the terminal | PASS |
| `SUZU-QA-TUI-002` | Render reviewed 80x24 and 120x32 buffers plus the below-minimum fallback; require Library on the left, Now Playing above Art in the middle, Queue on the right, centered stationary panel titles, and focus visible without depending on color | PASS (automated snapshots) |
| `SUZU-QA-TUI-003` | Enter the terminal guard through an injected output/raw-mode control, unwind with a panic, and require alternate-screen exit plus raw-mode restoration | PASS (automated unit check) |
| `SUZU-QA-LOG-001` | Force two-record byte rotation through the installed logging pipeline; require visible bounded truncation, current/archive `0600` files, removal of archives above a lowered retention bound, no terminal sink, and nonblocking refusal of a FIFO log entry | PASS |
| `SUZU-QA-LOG-002` | Replace the current log identity after the real TUI starts; require the background rotation failure to make the command fail and appear only after alternate-screen restoration | PASS |
| `SUZU-QA-LOG-003` | Hold a background write, start logging shutdown, and require the guard to remain pending until the owned worker is released and joined | PASS (automated) |
| `SUZU-QA-LOCK-002` | Start a second terminal identity while the first is active, then exercise normal exit, startup error, and unwinding panic; require both active-TUI and root leases to release on every destructor path | PASS |
| `SUZU-QA-FD-001` | Set `runtime.max_open_files` below the five-descriptor terminal startup peak; require refusal before runtime storage, leases, or a log file are opened | PASS (automated) |
| `SUZU-QA-ROOT-002` | Select a root, rename it, replace its old path with another valid root, then load config and scan; require both operations to remain on the selected descriptor and identity | PASS (automated) |

Physical audio devices, playback, packaged artifacts, network syscall
observation, and later desktop/status/mutation surfaces are not present and are
not claimed by these cases.
