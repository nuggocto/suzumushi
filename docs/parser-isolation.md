# Audio parser isolation

Status: accepted for the first playback pipeline.

## Decision

Suzumushi uses one pipeline:

```text
verified file descriptor -> Symphonia 0.6.0 helper -> bounded f32 PCM ->
owned worker -> rtrb 0.3.4 -> CPAL 0.18.1 -> Linux audio device
```

Symphonia is pure Rust and covers the four formats wanted for the first
playable build without system codec libraries. CPAL gives Suzumushi direct
control of the realtime callback. Rodio is not kept as a second path.

The decoder receives the already verified media descriptor as standard input.
It never receives or reopens a pathname. It is a child process because a
decoder thread cannot be forcibly stopped if malformed input makes third-party
code hang.

## Bounds and lifecycle

- The helper is limited to 256 MiB of address space, 600 seconds of total CPU,
  16 file descriptors, and no core dumps.
- Native FLAC input is descriptor-filtered so Symphonia sees StreamInfo but skips
  optional metadata blocks. Playback therefore never allocates FLAC tags,
  attachments, chapters, seek tables, or embedded pictures.
- Leading ID3v2 data is similarly hidden from the playback decoder; only the
  MPEG audio frames are relevant to this path.
- A parent-death signal prevents an orphaned helper from surviving the terminal.
- The parent stops and reaps a helper after three seconds without output
  progress, on track replacement, on stop, and during shutdown.
- PCM framing accepts at most 32,768 samples per block and two queued blocks.
  Error text is capped at 1,024 bytes.
- The current decoder accepts mono or stereo PCM at 8 kHz through 192 kHz. The
  output device must support the source sample rate. No hidden resampler or
  second fallback pipeline exists.
- The output ring is fixed at 262,144 samples. The callback pops samples,
  applies one atomic gain, writes silence on underrun, converts sample types,
  updates the fixed 16-band spectrum analyzer, and publishes atomic state. It
  does not allocate, block, log, take a lock, or send a message.
- Seeking restarts the isolated helper before the requested source timestamp,
  then discards decoded preroll against packet presentation timestamps before
  emitting PCM. Automatic session resume enters through the same verified
  descriptor and accurate-seek path; restoring state never reopens a saved
  pathname.
- Position updates use a capacity-one latest-value lane. Timeline revisions
  prevent reliable restart events and coalesced positions from being applied
  out of order.
- The app loop owns queue choice and playback generations. The worker reports
  completion and never chooses the next track, shuffle order, or repeat action.

## Verified compatibility

The checked-in, original sine-wave fixtures prove these combinations through
the selected Symphonia adapter at 48 kHz stereo:

| Container | Codec |
| --- | --- |
| MP3 | MPEG Layer III |
| FLAC | FLAC |
| WAV | signed 16-bit PCM |
| Ogg | Vorbis |

The fixtures, commands, license, and hashes are recorded in
`tests/fixtures/audio/SOURCES.md`. Extensions remain scanner hints. AAC, M4A,
Opus, unusual channel layouts, and unsupported device sample rates are not
compatibility claims.

## Verification

Unit tests cover the compatibility fixtures, preroll-free accurate seek through
all four containers, malformed input, helper timeout and reaping, the malformed
FLAC picture-length reproducer, verified descriptor reopening, deterministic
fake-device controls, device draining, stale generations, and queue navigation. The
`audio_decoder` fuzz target exercises the same Symphonia adapter with bounded
input and decoded output. A real Linux device check remains required whenever
the playback dependencies or output path changes.
