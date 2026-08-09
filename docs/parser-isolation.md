# Metadata Parser Isolation

## Selected parser

- Crate: `lofty = 0.24.0`
- Features: `default-features = false`; no optional features enabled
- API: `Probe<BufReader<BudgetedReader<File>>>::guess_file_type().read()` with
  `ParseOptions::read_properties(false)`, followed by `TaggedFileExt` and tag
  accessors for artist, album artist, album, and title
- Input: a duplicated, already opened and `fstat`-verified regular descriptor;
  the parser never receives a pathname

Lofty does not expose a caller-controlled work counter or cancellation point for
all supported tag formats. It therefore never runs in the scanner process.
Suzumushi starts the hidden `__metadata-helper` mode of the same canonical
executable, passes the descriptor as standard input, and waits for a bounded
JSON reply.

## Enforced bounds

| Resource | Bound |
|---|---:|
| Cumulative parser reads | 8 MiB |
| Parser read/seek operations | 4,096 |
| Seek range | Verified input length |
| Helper stdout | 64 KiB plus one-byte refusal probe |
| CPU rlimit | 2 seconds |
| Address-space rlimit | 256 MiB |
| Open-file rlimit | 16 |
| Core-file rlimit | 0 |
| Created-file-size rlimit | 1 MiB |
| Parent wall timeout | 3 seconds |
| Concurrent helpers | One in the current sequential scanner, below configured `max_parser_helpers` of two |

The output reader is bounded before allocation. On timeout or wait failure the
parent sends a kill, waits for the exact child, and joins the bounded output
reader. Parser errors, malformed replies, abnormal exits, exhausted budgets,
and panics become structured metadata warnings; track discovery keeps bounded
filename/path fallback fields.

## Throughput tradeoff

Each unique media asset starts a fresh helper process. Entries that resolve to
the same verified asset reuse its metadata, but the current scanner runs unique
asset helpers sequentially. Scan time therefore grows linearly with unique
assets and process startup is paid once per parser attempt.

The default 50,000-attempt limit and three-second wall timeout permit 150,000
seconds of configured parser wait when every helper times out, before process
startup, scheduling delay, and other scan work. The per-helper timeout is a
safety ceiling, not a global scan deadline or throughput estimate. No production
throughput claim exists yet. Batching, helper reuse, or added concurrency must
preserve descriptor-only input, bounded IPC and resources, and reliable
kill-and-reap behavior before replacing this isolation model.

The `metadata_adapter` fuzz target exercises the bounded reader and JSON frame
under pinned input, per-input timeout, total-time, maximum-length, and RSS
limits. A parser version, enabled feature, API, helper limit, or new parser
surface requires an update to this decision and `SECURITY.md` before use.
