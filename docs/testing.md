# Executable QA Record

The cases here cover only the currently runnable root and scanner surface. Each
run uses a fresh local temporary directory and removes it from the workspace
after evidence is checked.

Last verdict: **PASS** on 2026-08-09 (Europe/Paris), from the working tree based
on commit `20a5f71135597cbe71dd3aeb9218f025c2deaab4` on branch `shrek`. The tested
debug executable SHA-256 was
`22fed38da3161e7fb21a3f25bdb019fbd548f3621ed0fee5a84f183bdd30eaa7`.
`mise run ci` also passed with Rust 1.97.1, the Rust 1.95.0 MSRV, dependency
policy checks, 38 automated tests, and all four bounded fuzz smoke targets.

## Automated test layout

Pure module behavior stays in `#[cfg(test)]` modules beside its implementation.
The root `tests/` directory contains only crate-boundary tests: executable flows,
filesystem behavior, kernel locking, root discovery, and scanner behavior over
real temporary directories. Untrusted parser and projection inputs remain in
the dedicated `fuzz/` targets.

| Case | User flow and oracle | Evidence status |
|---|---|---|
| `SUZU-QA-ROOT-001` | Run `suzumushi init <fresh-root>`; inspect the full tree, instructions, `0700` private directories, and `0600` config | PASS |
| `SUZU-QA-SCAN-001` | Add copied and symlinked audio, local/shared lyrics, artwork, malformed and broken entries; run `diagnose --root`; require bounded counts, metadata/fallback output, and warnings without panic | PASS |
| `SUZU-QA-SCAN-002` | Add `audio/.git/index`, lower `max_entries` to the visible-tree count plus the hidden directory entry, and run `diagnose --root`; require `complete: true` with no descendant budget charge or warning | PASS |
| `SUZU-QA-CLI-001` | Run incomplete `suzumushi init`; require exit 2, empty stdout, and one clean `error:` prefix with no escaped newline | PASS |
| `SUZU-QA-FS-001` | Exercise external, broken, magic, directory, and non-regular symlink targets; require verified regular descriptors or explicit bounded refusal | PASS (CLI smoke plus automated race/magic-link coverage) |
| `SUZU-QA-FIFO-001` | Put FIFOs at a media target, `config.toml`, and an expected README path; require media diagnostics to warn, config/init to refuse clearly, and every command to return within the two-second test bound | PASS |
| `SUZU-QA-INIT-002` | Replace a nested expected directory with a file; require repeated initialization to report the exact nested path | PASS (automated) |
| `SUZU-QA-ORDER-001` | Diagnose nested `library/a/x.mp3` and `library/a-/x.mp3`; require component-wise order with `a` before `a-` | PASS |
| `SUZU-QA-PATH-001` | Diagnose one invalid-byte filename and one literal `\\xFF` filename; require distinct terminal-safe output with no raw control bytes | PASS |
| `SUZU-QA-MEM-001` | Reserve the old and replacement indexes plus scanner/parser scratch; accept the reviewed partition and reject one byte above it before traversal | PASS (automated) |
| `SUZU-QA-LOCK-001` | Hold root and per-user leases; require second owners to fail while bounded readers continue, then verify release and stale-file reuse | PASS (automated; diagnostics confirmed lock-free) |

Physical audio devices, playback, the TUI, packaged artifacts, network syscall
observation, and later desktop/status/mutation surfaces are not present and are
not claimed by these cases.
