# Audio fixtures

The four `tone` files are an original 440 Hz sine wave generated for this
project. They contain no third-party recording and are covered by the project's
Apache-2.0 license.

Generated with FFmpeg n8.1.2:

```sh
ffmpeg -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=0.25' -ac 2 -c:a pcm_s16le tone.wav
ffmpeg -i tone.wav -c:a flac tone.flac
ffmpeg -i tone.wav -c:a libmp3lame -b:a 128k tone.mp3
ffmpeg -i tone.wav -c:a libvorbis -q:a 4 tone.ogg
```

SHA-256:

```text
d91ab665afc30f802334b1a04f4e586b76f9b667f523528a9067a5fa2e69a5b5  tone.flac
569086b7abf7857ee447bb3ac64632fa3772ec9c93fc5312f6b8bbf0e248d169  tone.mp3
7b1b566db633b43b19144d58c3226d2c36cba4dd30dada0f8436f443a665816e  tone.ogg
ba3f76b18da556862f383f2e2504915e99019030138f05b047221e97b06781ae  tone.wav
```
