# Audio fixtures

The `tone` files are an original 440 Hz sine wave generated for this project.
They contain no third-party recording and are covered by the project's
Apache-2.0 license.

Generated with FFmpeg n8.1.2:

```sh
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=0.25' -ac 2 -c:a pcm_s16le tone.wav
ffmpeg -i tone.wav -c:a flac tone.flac
ffmpeg -i tone.wav -c:a libmp3lame -b:a 128k tone.mp3
ffmpeg -i tone.wav -c:a libvorbis -q:a 4 tone.ogg
```

`tone-44100.wav` is the same tone at 44.1 kHz. Every other fixture is 48 kHz,
which once matched the usual PipeWire graph rate exactly and left rate handling
untested; this file keeps a non-48 kHz track in the suite. Generated with FFmpeg
n9.0.1:

```sh
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=44100:duration=0.25' -ac 2 -c:a pcm_s16le tone-44100.wav
```

`invalid-wav-channels.bin` is a 60-byte synthetic WAV header produced by the
decoder fuzzer. Its channel count overflows Symphonia 0.6.1's unchecked block
alignment multiplication. It is retained as a regression fixture and as
`fuzz/corpus/audio_decoder/seed-invalid-wav-channels`.

SHA-256:

```text
fc0950913760e04414269cc245c9ee6cf85455d9ff85ccf42a50905bd6b79247  invalid-wav-channels.bin
39c9c594c0481ca3d26035c79d09c48a17eed1b58a088639511d3fbc29256784  tone-44100.wav
d91ab665afc30f802334b1a04f4e586b76f9b667f523528a9067a5fa2e69a5b5  tone.flac
569086b7abf7857ee447bb3ac64632fa3772ec9c93fc5312f6b8bbf0e248d169  tone.mp3
7b1b566db633b43b19144d58c3226d2c36cba4dd30dada0f8436f443a665816e  tone.ogg
ba3f76b18da556862f383f2e2504915e99019030138f05b047221e97b06781ae  tone.wav
```
