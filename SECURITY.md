# Security Policy

## Reporting a vulnerability

Please do not open a public issue with vulnerability details. Use GitHub's
private vulnerability reporting for this repository. If that option is not
available, open an issue containing only a request for private contact and no
technical details, secrets, personal data, or proof-of-concept material.

Include the affected revision, a concise description of the impact, the
conditions needed to reproduce it, and a minimal safe reproduction when
possible. The maintainer will acknowledge the report, assess its scope, and
coordinate a fix and disclosure.

## Supported versions

Suzumushi is still in development. Security fixes apply to the current
development line. Older snapshots are not maintained separately.

## Current security review

This register covers the trust boundaries currently reachable through root
initialization, `diagnose`, and the terminal shell. The security owner is the
Suzumushi maintainer.

| Asset and boundary | Attacker capability and entry point | Mitigation | Verification | Residual risk and review trigger |
|---|---|---|---|---|
| User-owned root, private config/state/log/backup paths | A local user or compromised directory supplies symlinks, substituted parents, special files, foreign ownership, broad modes, or a concurrent writer | Refuse a symlinked final root, pin directory descriptors, use no-follow relative opens, probe unknown file entries nonblockingly before requiring a regular descriptor, require current UID, enforce `0700` directories plus `0600` private files, and acquire the verified descriptor-relative root lease before initialization creates or repairs owned entries | `root_init` ownership, mode, unrelated-content, symlink, unsafe-lock, held-lease, and no-mutation-on-contention tests plus executable FIFO config/README regressions | A hostile privileged process can still alter another process's files. Review before any new mutation path. |
| Audio traversal | Media names, directory entries, mounts, and symlinks change during a scan | Pin `audio` once; enumerate each admitted directory descriptor once in deterministic component-wise natural order; descend only into `library` and `playlists`; skip hidden directory subtrees when configured; open unknown file targets nonblockingly and verify the retained descriptor is regular; never descend through directory symlinks; use `openat2(RESOLVE_NO_MAGICLINKS)` for file symlinks, record external/cross-mount identity, and return `unsupported_secure_open` without a pathname fallback | Scanner substitution, retarget, FIFO, root-level hidden and unrelated-directory budget, magic-link, external/broken/directory-symlink, component-order, mount, one-traversal, and no-reopen tests | Network and userspace filesystems may have unusual semantics and fail closed. Review when filesystem support changes. |
| Process memory and work | A root contains many, deep, long, duplicate, malformed, or cross-mount entries, or a PTY reports crafted dimensions | Checked entry/file/depth/playlist/symlink/parser/path/metadata/warning/index limits; reserve active plus replacement indexes before traversal; check both Ratatui cell buffers against the 8 MiB UI/state reservation before initial allocation and every fixed-viewport replacement; return a visibly partial scan or fail the oversized terminal before growth | Config and scanner limit-plus-one tests, replacement reservation test, terminal exact-bound unit test, oversized real-PTY resize/restoration regression, bounded fuzz tasks | Index accounting is conservative app-owned accounting, not whole-process RSS measurement. Allocator and dependency overhead outside the reserved models still exists. Review when models or Ratatui cell storage gain allocations. |
| Metadata parser | A local audio file supplies malformed tags, lengths, nesting, seeks, or parser hangs/crashes | Run Lofty only in the hidden helper mode described in `docs/parser-isolation.md`; give it a verified descriptor, bounded reader, bounded JSON reply, rlimits, wall timeout, kill, and reap | Real tagged CLI fixture; malformed adapter fuzzing; explicit helper crash and timeout cleanup tests | Sequential per-asset helpers make total scan latency grow with unique assets, and per-helper timeouts do not create a global scan deadline. Kernel scheduling can also delay timeout observation by one polling interval. Review every Lofty/API/feature change, helper lifecycle change, and unsafe/FFI addition. |
| Terminal diagnostics | Filenames, tags, parser errors, or even selected root paths contain control bytes, invalid UTF-8, or excessive text | Accept native Linux paths, project every rendered value through the terminal-specific sanitizer, reserve unambiguous backslash escapes for invalid/control bytes and literal backslashes, and cap retained fields/warnings before printing | Terminal-safe display fuzz target, raw-escape CLI regression, invalid-byte/literal-escape collision regression, non-UTF-8 executable test, and bounded scanner models | Other formatting languages are not present yet. Review separately when Waybar, tmux, Zellij, notifications, or file URIs arrive. |
| Root and active-TUI locks | A local process leaves stale files, races initialization or another writer, or forges insecure runtime storage | Kernel `flock` owns liveness; never delete a stale file blindly; record PID/start ticks only after acquisition; initialization verifies and acquires the pinned root lock before root writes; keep the global file below verified current-UID `XDG_RUNTIME_DIR` with no `/tmp` fallback | Root exclusivity, reinitialization contention and unsafe-lock refusal, stale-file reuse, missing/insecure runtime, and same-UID contention tests | PID text is diagnostic, while the held kernel lock is authoritative. Review when process liveness becomes a persisted status contract. |
| Terminal lifecycle and capability replies | A terminal sends malformed, oversized, absent, or delayed control replies or dimensions, or the process exits while raw mode and the alternate screen are active | Check initial and resized cell buffers before allocation; enter the alternate screen before probing; select identified Kitty-family/iTerm2 terminals without a reply; otherwise keep one device-attributes query outstanding; cap replies at 4 KiB and 150 ms; accept only a complete frame; fail on a partial deadline; permit `q`/Ctrl-C cancellation; skip queries through tmux/Zellij; keep a restoration guard across every error and unwind | Capability parser unit/fuzz coverage plus pseudo-terminal no-reply fallback, bounded and oversized real resize delivery, clean quit, and normal/unwinding escape-restoration verification | A terminal that stays entirely silent past the reply deadline and sends a complete reply later violates the bounded exchange and may still confuse its own input stream. `abort`, `SIGKILL`, and power loss cannot run destructors. The user may need `reset` or `stty sane`; the kernel releases held locks for the next start. Review before accepting arbitrary terminal query payloads or rendering images. |
| Private diagnostic logs | A root substitutes a symlink, FIFO, device, hard link, broad-mode file, or oversized diagnostic at a log path, or storage fails after startup | Open relative to the verified private log directory with no-follow and nonblocking flags; require current UID, regular single-link `0600` files; bound formatted records and the lossy queue; rotate only fixed app-owned names by byte and file count; prune archives above a lowered retention bound; track background write/flush/rotation failures; close admission, drain, and join the owned worker before reporting; report final queue drops outside the lossy queue after terminal restoration; never send logs to the TUI | FIFO executable regression, private-mode, lowered-retention, installed-pipeline truncation, held-writer join test, background identity-change failure, process-memory/config contradiction, and pseudo-terminal transcript checks | Same-UID or privileged processes can still interfere with user-owned storage. A stuck local filesystem write delays the awaited shutdown rather than being detached. Dropped logs reduce diagnostics by design but are counted and reported at shutdown. Review before logging untrusted fields or adding another sink. |
| Dependencies | A vulnerable, unlicensed, duplicated, or non-registry crate enters the runtime graph | Exact direct versions, committed locks, pinned `cargo audit` and `cargo deny` tasks, explicit accepted licenses and sources | `mise run security` | Advisory databases are external and point-in-time. Review on every dependency update. |

There is no IPv4/IPv6 feature, listener, remote API, credential, or secret in the
current product. Local audio and D-Bus sockets are not used by this work.

## Temporary advisory exception

`RUSTSEC-2024-0436` reports that `paste 1.0.15` is unmaintained. It enters only
through `lofty 0.24.0`; the advisory does not report a vulnerability. The
Suzumushi maintainer owns this exception through 2026-11-09. It must be removed
sooner if Lofty drops `paste`, a maintained compatible parser is selected, or a
security advisory affects `paste`. All other audit warnings remain denied.
