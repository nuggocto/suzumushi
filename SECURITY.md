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
initialization and `diagnose`. The security owner is the Suzumushi maintainer.

| Asset and boundary | Attacker capability and entry point | Mitigation | Verification | Residual risk and review trigger |
|---|---|---|---|---|
| User-owned root, private config/state/log/backup paths | A local user or compromised directory supplies symlinks, substituted parents, special files, foreign ownership, or broad modes | Refuse a symlinked final root, pin directory descriptors, use no-follow relative opens, probe unknown file entries nonblockingly before requiring a regular descriptor, require current UID, and enforce `0700` directories plus `0600` private files | `root_init` ownership, mode, unrelated-content, and symlink tests plus executable FIFO config/README regressions | A hostile privileged process can still alter another process's files. Review before any new mutation path. |
| Audio traversal | Media names, directory entries, mounts, and symlinks change during a scan | Pin `audio` once; enumerate each directory descriptor once in deterministic component-wise natural order; skip hidden directory subtrees when configured; open unknown file targets nonblockingly and verify the retained descriptor is regular; never descend through directory symlinks; use `openat2(RESOLVE_NO_MAGICLINKS)` for file symlinks, record external/cross-mount identity, and return `unsupported_secure_open` without a pathname fallback | Scanner substitution, retarget, FIFO, root-level hidden-directory budget, magic-link, external/broken/directory-symlink, component-order, mount, one-traversal, and no-reopen tests | Network and userspace filesystems may have unusual semantics and fail closed. Review when filesystem support changes. |
| Process memory and work | A root contains many, deep, long, duplicate, malformed, or cross-mount entries | Checked entry/file/depth/playlist/symlink/parser/path/metadata/warning/index limits; reserve active plus replacement indexes before traversal; return a visibly partial result at exhaustion | Config and scanner limit-plus-one tests, replacement reservation test, bounded fuzz tasks | Index accounting is conservative app-owned accounting, not whole-process RSS measurement. Review when models gain allocations. |
| Metadata parser | A local audio file supplies malformed tags, lengths, nesting, seeks, or parser hangs/crashes | Run Lofty only in the hidden helper mode described in `docs/parser-isolation.md`; give it a verified descriptor, bounded reader, bounded JSON reply, rlimits, wall timeout, kill, and reap | Real tagged CLI fixture; malformed adapter fuzzing; explicit helper crash and timeout cleanup tests | Sequential per-asset helpers make total scan latency grow with unique assets, and per-helper timeouts do not create a global scan deadline. Kernel scheduling can also delay timeout observation by one polling interval. Review every Lofty/API/feature change, helper lifecycle change, and unsafe/FFI addition. |
| Terminal diagnostics | Filenames, tags, parser errors, or even selected root paths contain control bytes, invalid UTF-8, or excessive text | Accept native Linux paths, project every rendered value through the terminal-specific sanitizer, reserve unambiguous backslash escapes for invalid/control bytes and literal backslashes, and cap retained fields/warnings before printing | Terminal-safe display fuzz target, raw-escape CLI regression, invalid-byte/literal-escape collision regression, non-UTF-8 executable test, and bounded scanner models | Other formatting languages are not present yet. Review separately when Waybar, tmux, Zellij, notifications, or file URIs arrive. |
| Root and active-TUI locks | A local process leaves stale files, races a writer, or forges insecure runtime storage | Kernel `flock` owns liveness; never delete a stale file blindly; record PID/start ticks only after acquisition; keep the global file below verified current-UID `XDG_RUNTIME_DIR` with no `/tmp` fallback | Root exclusivity, stale-file reuse, missing/insecure runtime, and same-UID contention tests | PID text is diagnostic, while the held kernel lock is authoritative. Review when process liveness becomes a persisted status contract. |
| Dependencies | A vulnerable, unlicensed, duplicated, or non-registry crate enters the runtime graph | Exact direct versions, committed locks, pinned `cargo audit` and `cargo deny` tasks, explicit accepted licenses and sources | `mise run security` | Advisory databases are external and point-in-time. Review on every dependency update. |

There is no IPv4/IPv6 feature, listener, remote API, credential, or secret in the
current product. Local audio and D-Bus sockets are not used by this work.

## Temporary advisory exception

`RUSTSEC-2024-0436` reports that `paste 1.0.15` is unmaintained. It enters only
through `lofty 0.24.0`; the advisory does not report a vulnerability. The
Suzumushi maintainer owns this exception through 2026-11-09. It must be removed
sooner if Lofty drops `paste`, a maintained compatible parser is selected, or a
security advisory affects `paste`. All other audit warnings remain denied.
