// SPDX-License-Identifier: Apache-2.0

//! Symphonia decoding isolated behind a bounded helper protocol.

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::os::fd::AsFd;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::process::{Pid, Resource, Rlimit, Signal, setrlimit};
use symphonia::core::audio::GenericAudioBufferRef;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{
    FormatOptions, FormatReader, SeekMode, SeekTo, SeekedTo, TrackType,
};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::units::{Time as MediaTime, TimeBase, Timestamp};

use super::{AudioFormat, DecodedInfo, DecoderPoll, DecoderStream};

const READY_MAGIC: &[u8; 8] = b"SUZPCM02";
const ERROR_MAGIC: &[u8; 8] = b"SUZERR01";
const ERROR_FRAME: u32 = u32::MAX;
const DECODER_MESSAGES: usize = 2;
const MAX_PCM_BLOCK_SAMPLES: usize = 32_768;
const MAX_ERROR_BYTES: usize = 1_024;
const MAX_CONSECUTIVE_DECODE_ERRORS: usize = 32;
const MAX_FLAC_METADATA_BLOCKS: usize = 1_024;
const MAX_WAV_HEADER_CHUNKS: usize = 1_024;
const HELPER_NO_PROGRESS: Duration = Duration::from_secs(3);
const FUZZ_SAMPLE_LIMIT: u64 = 1_000_000;
const UNKNOWN_DURATION_MICROS: u64 = u64::MAX;
const MAX_TRACK_DURATION: Duration = Duration::from_hours(366 * 24);

pub(super) struct HelperDecoder {
    child: Option<Child>,
    messages: Option<Receiver<DecoderPoll>>,
    reader: Option<JoinHandle<()>>,
}

impl HelperDecoder {
    pub(super) fn start(file: &File, position: Duration) -> Result<Self, String> {
        let executable = std::env::current_exe()
            .map_err(|error| format!("cannot locate the decoder helper: {error}"))?;
        let mut command = Command::new(executable);
        command
            .arg("__audio-decode-helper")
            .arg(position.as_micros().min(u128::from(u64::MAX)).to_string());
        Self::start_with(command, file, HELPER_NO_PROGRESS)
    }

    fn start_with(
        mut command: Command,
        file: &File,
        no_progress: Duration,
    ) -> Result<Self, String> {
        let mut child = command
            .stdin(Stdio::from(file.try_clone().map_err(|error| {
                format!("cannot duplicate the media descriptor: {error}")
            })?))
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("cannot start the decoder helper: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "decoder helper stdout is unavailable".to_owned())?;
        let (message_tx, message_rx) = sync_channel(DECODER_MESSAGES);
        let reader = thread::Builder::new()
            .name("suzumushi-decoder-output".into())
            .spawn(move || read_protocol(stdout, &message_tx, no_progress))
            .map_err(|error| {
                let _ = child.kill();
                let _ = child.wait();
                format!("cannot start the decoder output reader: {error}")
            })?;
        Ok(Self {
            child: Some(child),
            messages: Some(message_rx),
            reader: Some(reader),
        })
    }
}

impl DecoderStream for HelperDecoder {
    fn poll(&mut self) -> DecoderPoll {
        let Some(messages) = self.messages.as_ref() else {
            return DecoderPoll::Failed("decoder helper is already stopped".into());
        };
        match messages.try_recv() {
            Ok(message) => message,
            Err(TryRecvError::Empty) => DecoderPoll::Pending,
            Err(TryRecvError::Disconnected) => {
                DecoderPoll::Failed("decoder helper output ended without a final frame".into())
            }
        }
    }

    fn shutdown(&mut self) -> Result<(), String> {
        let mut failures = Vec::new();
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(Some(_)) => {}
                Ok(None) => {
                    if let Err(error) = child.kill()
                        && error.kind() != std::io::ErrorKind::InvalidInput
                    {
                        failures.push(format!("cannot kill decoder helper: {error}"));
                    }
                }
                Err(error) => failures.push(format!("cannot inspect decoder helper: {error}")),
            }
            if let Err(error) = child.wait() {
                failures.push(format!("cannot reap decoder helper: {error}"));
            }
        }
        self.child.take();
        self.messages.take();
        if self
            .reader
            .take()
            .is_some_and(|reader| reader.join().is_err())
        {
            failures.push("decoder output reader panicked".into());
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    }
}

impl Drop for HelperDecoder {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

fn read_protocol(mut stdout: ChildStdout, messages: &SyncSender<DecoderPoll>, timeout: Duration) {
    let result = read_protocol_inner(&mut stdout, messages, timeout);
    if let Err(error) = result {
        let _ = messages.send(DecoderPoll::Failed(error));
    }
}

fn read_protocol_inner(
    stdout: &mut ChildStdout,
    messages: &SyncSender<DecoderPoll>,
    timeout: Duration,
) -> Result<(), String> {
    let mut magic = [0_u8; 8];
    read_exact_with_progress(stdout, &mut magic, timeout)?;
    if &magic == ERROR_MAGIC {
        return Err(read_error(stdout, timeout)?);
    }
    if &magic != READY_MAGIC {
        return Err("decoder helper returned an unknown protocol header".into());
    }
    let mut header = [0_u8; 24];
    read_exact_with_progress(stdout, &mut header, timeout)?;
    let format = AudioFormat {
        sample_rate: u32::from_le_bytes(header[0..4].try_into().expect("four-byte rate")),
        channels: u16::from_le_bytes(header[4..6].try_into().expect("two-byte channels")),
    };
    if header[6..8] != [0, 0] {
        return Err("decoder helper format header has non-zero reserved bytes".into());
    }
    let duration_micros =
        u64::from_le_bytes(header[8..16].try_into().expect("eight-byte duration"));
    let position_micros =
        u64::from_le_bytes(header[16..24].try_into().expect("eight-byte position"));
    let duration = (duration_micros != UNKNOWN_DURATION_MICROS)
        .then(|| Duration::from_micros(duration_micros));
    if duration.is_some_and(|value| value > MAX_TRACK_DURATION) {
        return Err("decoder helper reported a track longer than one year".into());
    }
    let position = Duration::from_micros(position_micros);
    if position > MAX_TRACK_DURATION || duration.is_some_and(|value| position > value) {
        return Err("decoder helper reported an invalid playback position".into());
    }
    messages
        .send(DecoderPoll::Ready(DecodedInfo {
            format,
            duration,
            position,
        }))
        .map_err(|_| "decoder consumer stopped".to_owned())?;

    loop {
        let mut length = [0_u8; 4];
        read_exact_with_progress(stdout, &mut length, timeout)?;
        let sample_count = u32::from_le_bytes(length);
        if sample_count == 0 {
            messages
                .send(DecoderPoll::End)
                .map_err(|_| "decoder consumer stopped".to_owned())?;
            return Ok(());
        }
        if sample_count == ERROR_FRAME {
            return Err(read_error(stdout, timeout)?);
        }
        let sample_count = usize::try_from(sample_count)
            .map_err(|_| "decoder PCM block length does not fit memory".to_owned())?;
        if sample_count > MAX_PCM_BLOCK_SAMPLES {
            return Err(format!(
                "decoder PCM block exceeded {MAX_PCM_BLOCK_SAMPLES} samples"
            ));
        }
        let byte_count = sample_count
            .checked_mul(size_of::<f32>())
            .ok_or_else(|| "decoder PCM block length overflow".to_owned())?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(byte_count)
            .map_err(|error| format!("cannot reserve decoder PCM block: {error}"))?;
        bytes.resize(byte_count, 0);
        read_exact_with_progress(stdout, &mut bytes, timeout)?;
        let mut samples = Vec::new();
        samples
            .try_reserve_exact(sample_count)
            .map_err(|error| format!("cannot reserve decoded samples: {error}"))?;
        for chunk in bytes.as_chunks::<4>().0 {
            samples.push(f32::from_le_bytes(*chunk));
        }
        messages
            .send(DecoderPoll::Samples(samples))
            .map_err(|_| "decoder consumer stopped".to_owned())?;
    }
}

fn read_exact_with_progress(
    reader: &mut ChildStdout,
    mut buffer: &mut [u8],
    timeout: Duration,
) -> Result<(), String> {
    let seconds = i64::try_from(timeout.as_secs()).unwrap_or(i64::MAX);
    let nanoseconds = i64::from(timeout.subsec_nanos());
    let timeout = Timespec {
        tv_sec: seconds,
        tv_nsec: nanoseconds,
    };
    while !buffer.is_empty() {
        let mut descriptor = [PollFd::new(reader, PollFlags::IN)];
        let ready = poll(&mut descriptor, Some(&timeout))
            .map_err(|error| format!("cannot poll decoder helper output: {error}"))?;
        if ready == 0 {
            return Err("decoder helper made no output progress and was stopped".into());
        }
        match reader.read(buffer) {
            Ok(0) => return Err("decoder helper output ended early".into()),
            Ok(read) => buffer = &mut buffer[read..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("cannot read decoder helper output: {error}")),
        }
    }
    Ok(())
}

fn read_error(reader: &mut ChildStdout, timeout: Duration) -> Result<String, String> {
    let mut length = [0_u8; 2];
    read_exact_with_progress(reader, &mut length, timeout)?;
    let length = usize::from(u16::from_le_bytes(length));
    if length == 0 || length > MAX_ERROR_BYTES {
        return Err("decoder helper returned an invalid error length".into());
    }
    let mut bytes = vec![0_u8; length];
    read_exact_with_progress(reader, &mut bytes, timeout)?;
    String::from_utf8(bytes).map_err(|_| "decoder helper returned a non-UTF-8 error".into())
}

/// Runs the internal decoder helper on its inherited standard input descriptor.
#[must_use]
pub fn helper_main(position_micros: u64) -> i32 {
    let mut sink = ProtocolSink::new(std::io::stdout().lock());
    if let Err(error) = apply_helper_limits() {
        let _ = sink.error(&error);
        return 1;
    }
    let stdin = std::io::stdin();
    let fd = match rustix::io::dup(stdin.as_fd()) {
        Ok(fd) => fd,
        Err(error) => {
            let _ = sink.error(&format!("cannot duplicate input descriptor: {error}"));
            return 1;
        }
    };
    let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        decode_source(
            File::from(fd),
            &mut sink,
            None,
            Duration::from_micros(position_micros),
        )
    }));
    let result = match decoded {
        Ok(Ok(())) => sink.finish(),
        Ok(Err(error)) => sink.error(&error),
        Err(_) => sink.error("audio decoder panicked"),
    };
    i32::from(result.is_err())
}

fn apply_helper_limits() -> Result<(), String> {
    rustix::process::set_parent_process_death_signal(Some(Signal::KILL))
        .map_err(|error| format!("cannot bind decoder lifetime to its parent: {error}"))?;
    if rustix::process::getppid().is_none_or(Pid::is_init) {
        return Err("decoder parent exited during helper startup".into());
    }
    for (resource, value) in [
        (Resource::Cpu, 600),
        (Resource::As, 256 * 1_048_576),
        (Resource::Nofile, 16),
        (Resource::Core, 0),
    ] {
        setrlimit(
            resource,
            Rlimit {
                current: Some(value),
                maximum: Some(value),
            },
        )
        .map_err(|error| format!("cannot apply decoder resource limit: {error}"))?;
    }
    Ok(())
}

trait PcmSink {
    fn ready(&mut self, info: DecodedInfo) -> Result<(), String>;
    fn samples(&mut self, samples: &[f32]) -> Result<(), String>;
}

struct PlaybackSource<S> {
    inner: S,
    container: AudioContainer,
    hidden_flac_headers: Vec<u64>,
}

#[derive(Clone, Copy)]
enum AudioContainer {
    Flac,
    Mp3,
    Ogg,
    Wav,
}

impl<S: MediaSource> PlaybackSource<S> {
    fn new(mut inner: S) -> Result<Self, String> {
        inner
            .seek(SeekFrom::Start(0))
            .map_err(|error| format!("cannot rewind audio source: {error}"))?;
        let mut prefix = Vec::new();
        prefix
            .try_reserve_exact(12)
            .map_err(|error| format!("cannot reserve audio format probe: {error}"))?;
        (&mut inner)
            .take(12)
            .read_to_end(&mut prefix)
            .map_err(|error| format!("cannot inspect audio source: {error}"))?;
        let id3_len = id3v2_prefix_len(&prefix, inner.byte_len())?;
        let (container, data_offset) = if let Some(offset) = id3_len {
            inner
                .seek(SeekFrom::Start(offset))
                .map_err(|error| format!("cannot skip ID3 data: {error}"))?;
            let mut header = [0_u8; 4];
            inner
                .read_exact(&mut header)
                .map_err(|error| format!("cannot read MPEG audio header: {error}"))?;
            if !looks_like_mp3_header(&header) {
                return Err("ID3 data is not followed by an MPEG Layer III frame".into());
            }
            (AudioContainer::Mp3, offset)
        } else if prefix.starts_with(b"fLaC") {
            (AudioContainer::Flac, 0)
        } else if prefix.starts_with(b"OggS") {
            (AudioContainer::Ogg, 0)
        } else if prefix
            .get(..12)
            .is_some_and(|header| &header[..4] == b"RIFF" && &header[8..] == b"WAVE")
        {
            (AudioContainer::Wav, 0)
        } else if looks_like_mp3_header(&prefix) {
            (AudioContainer::Mp3, 0)
        } else {
            return Err("audio source is not a supported container".into());
        };

        let mut hidden_flac_headers = Vec::new();
        if matches!(container, AudioContainer::Wav) {
            validate_wav_channels(&mut inner)?;
        }
        if matches!(container, AudioContainer::Flac) {
            inner
                .seek(SeekFrom::Start(4))
                .map_err(|error| format!("cannot inspect FLAC metadata: {error}"))?;
            let mut found_last = false;
            for _ in 0..MAX_FLAC_METADATA_BLOCKS {
                let offset = inner
                    .stream_position()
                    .map_err(|error| format!("cannot inspect FLAC metadata position: {error}"))?;
                let mut header = [0_u8; 4];
                inner
                    .read_exact(&mut header)
                    .map_err(|error| format!("cannot read FLAC metadata header: {error}"))?;
                let is_last = header[0] & 0x80 != 0;
                let block_type = header[0] & 0x7f;
                let block_len = u32::from_be_bytes([0, header[1], header[2], header[3]]);

                // Playback needs StreamInfo only. Masking every optional block prevents the
                // decoder from allocating tags, attachments, chapters, or embedded pictures.
                if block_type != 0 {
                    hidden_flac_headers.push(offset);
                }
                inner
                    .seek(SeekFrom::Current(i64::from(block_len)))
                    .map_err(|error| format!("cannot skip FLAC metadata block: {error}"))?;
                if is_last {
                    found_last = true;
                    break;
                }
            }
            if !found_last {
                return Err(format!(
                    "FLAC metadata exceeded {MAX_FLAC_METADATA_BLOCKS} blocks"
                ));
            }
        }
        inner
            .seek(SeekFrom::Start(data_offset))
            .map_err(|error| format!("cannot position audio source: {error}"))?;
        Ok(Self {
            inner,
            container,
            hidden_flac_headers,
        })
    }
}

fn validate_wav_channels(source: &mut (impl Read + Seek)) -> Result<(), String> {
    source
        .seek(SeekFrom::Start(12))
        .map_err(|error| format!("cannot inspect WAV chunks: {error}"))?;
    for _ in 0..MAX_WAV_HEADER_CHUNKS {
        let mut header = [0_u8; 8];
        source
            .read_exact(&mut header)
            .map_err(|error| format!("cannot read WAV chunk header: {error}"))?;
        let length = u32::from_le_bytes(header[4..].try_into().expect("four length bytes"));
        if &header[..4] == b"data" {
            return Ok(());
        }
        let consumed = if &header[..4] == b"fmt " {
            if length < 4 {
                return Err("WAV format chunk is truncated".into());
            }
            let mut format = [0_u8; 4];
            source
                .read_exact(&mut format)
                .map_err(|error| format!("cannot read WAV channel count: {error}"))?;
            let channels = u16::from_le_bytes([format[2], format[3]]);
            // Symphonia 0.6.1 multiplies this untrusted u16 before validating it.
            // Apply our mono/stereo contract to every fmt chunk before parsing.
            if !(1..=2).contains(&channels) {
                return Err(format!("unsupported WAV channel count: {channels}"));
            }
            4
        } else {
            0
        };
        // RIFF chunks include one padding byte when their payload length is odd.
        let skip = i64::from(length) - consumed + i64::from(length & 1);
        source
            .seek(SeekFrom::Current(skip))
            .map_err(|error| format!("cannot skip WAV header chunk: {error}"))?;
    }
    Err(format!(
        "WAV header exceeded {MAX_WAV_HEADER_CHUNKS} chunks"
    ))
}

fn id3v2_prefix_len(prefix: &[u8], source_len: Option<u64>) -> Result<Option<u64>, String> {
    if !prefix.starts_with(b"ID3") {
        return Ok(None);
    }
    let header = prefix
        .get(..10)
        .ok_or_else(|| "truncated ID3 header".to_owned())?;
    if !(2..=4).contains(&header[3]) || header[4] == 0xff {
        return Err("unsupported ID3 version".into());
    }
    if header[6..10].iter().any(|byte| byte & 0x80 != 0) {
        return Err("invalid ID3 size".into());
    }
    let body_len = header[6..10]
        .iter()
        .fold(0_u64, |value, byte| (value << 7) | u64::from(*byte));
    let footer_len = u64::from(header[3] == 4 && header[5] & 0x10 != 0) * 10;
    let tag_len = 10_u64
        .checked_add(body_len)
        .and_then(|value| value.checked_add(footer_len))
        .ok_or_else(|| "ID3 size overflow".to_owned())?;
    if source_len.is_some_and(|length| tag_len > length) {
        return Err("ID3 tag extends beyond the audio source".into());
    }
    Ok(Some(tag_len))
}

fn looks_like_mp3_header(prefix: &[u8]) -> bool {
    let Some(header) = prefix.get(..4) else {
        return false;
    };
    let version = (header[1] >> 3) & 0b11;
    let layer = (header[1] >> 1) & 0b11;
    let bitrate = header[2] >> 4;
    let sample_rate = (header[2] >> 2) & 0b11;
    header[0] == 0xff
        && header[1] & 0xe0 == 0xe0
        && version != 0b01
        && layer == 0b01
        && !matches!(bitrate, 0 | 0x0f)
        && sample_rate != 0b11
}

impl<S: MediaSource> Read for PlaybackSource<S> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let start = self.inner.stream_position()?;
        let read = self.inner.read(buffer)?;
        let end = start.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        let first = self
            .hidden_flac_headers
            .partition_point(|offset| *offset < start);
        for offset in self.hidden_flac_headers[first..]
            .iter()
            .take_while(|offset| **offset < end)
        {
            let index = usize::try_from(*offset - start).expect("read buffer index fits usize");
            buffer[index] = (buffer[index] & 0x80) | 1;
        }
        Ok(read)
    }
}

impl<S: MediaSource> Seek for PlaybackSource<S> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

impl<S: MediaSource> MediaSource for PlaybackSource<S> {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len()
    }
}

fn decode_source<S: MediaSource + 'static>(
    source: S,
    sink: &mut dyn PcmSink,
    sample_limit: Option<u64>,
    position: Duration,
) -> Result<(), String> {
    let mut format = open_format(source)?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| "audio container has no default audio track".to_owned())?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|params| params.audio())
        .ok_or_else(|| "audio track has no decoder parameters".to_owned())?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|error| format!("cannot create audio decoder: {error}"))?;
    let track_id = track.id;
    let time_base = track.time_base;
    let duration = track_duration(track)?;
    let position = duration.map_or(position, |duration| position.min(duration));
    if position > MAX_TRACK_DURATION {
        return Err("requested playback position is longer than one year".into());
    }
    let seeked_to = if position.is_zero() {
        None
    } else {
        let seeked_to = seek_source(&mut *format, track_id, position)?;
        if seeked_to.actual_ts > seeked_to.required_ts {
            return Err("accurate seek returned a position after its target".into());
        }
        decoder.reset();
        Some(seeked_to)
    };
    let mut active_format = None;
    let mut seek_target = seeked_to.map(|seeked_to| seeked_to.required_ts);
    let mut scratch = Vec::<f32>::new();
    let mut total_samples = 0_u64;
    let mut decode_errors = 0_usize;
    while let Some(packet) = format
        .next_packet()
        .map_err(|error| format!("cannot read audio packet: {error}"))?
    {
        if packet.track_id != track_id {
            continue;
        }
        let packet_timestamp = packet.pts;
        let audio = match decoder.decode(&packet) {
            Ok(audio) => {
                decode_errors = 0;
                audio
            }
            Err(SymphoniaError::DecodeError(_))
                if decode_errors < MAX_CONSECUTIVE_DECODE_ERRORS =>
            {
                decode_errors += 1;
                continue;
            }
            Err(error) => return Err(format!("cannot decode audio packet: {error}")),
        };
        let current = validated_audio_format(&audio)?;
        match active_format {
            None => {
                sink.ready(DecodedInfo {
                    format: current,
                    duration,
                    position,
                })?;
                active_format = Some(current);
            }
            Some(expected) if expected != current => {
                return Err("audio format changed within one track".into());
            }
            Some(_) => {}
        }
        scratch.clear();
        scratch
            .try_reserve(audio.samples_interleaved())
            .map_err(|error| format!("cannot reserve decoded PCM scratch: {error}"))?;
        scratch.resize(audio.samples_interleaved(), 0.0);
        audio.copy_to_slice_interleaved(&mut scratch);
        let skip_samples = seek_sample_offset(
            &mut seek_target,
            packet_timestamp,
            time_base,
            current,
            scratch.len(),
        )?;
        for block in scratch[skip_samples..].chunks(MAX_PCM_BLOCK_SAMPLES) {
            total_samples = total_samples
                .checked_add(
                    u64::try_from(block.len())
                        .map_err(|_| "decoded sample count overflow".to_owned())?,
                )
                .ok_or_else(|| "decoded sample count overflow".to_owned())?;
            if sample_limit.is_some_and(|limit| total_samples > limit) {
                return Err("decoded sample limit reached".into());
            }
            sink.samples(block)?;
        }
    }
    if active_format.is_none() {
        return Err("track contained no decodable audio samples".into());
    }
    Ok(())
}

fn validated_audio_format(audio: &GenericAudioBufferRef<'_>) -> Result<AudioFormat, String> {
    let channels = u16::try_from(audio.spec().channels().count())
        .map_err(|_| "decoded channel count does not fit the protocol".to_owned())?;
    if channels == 0 || channels > 2 {
        return Err(format!(
            "decoded audio has {channels} channels; this build supports mono and stereo"
        ));
    }
    let sample_rate = audio.spec().rate();
    if !(8_000..=192_000).contains(&sample_rate) {
        return Err(format!(
            "decoded sample rate {sample_rate} Hz is outside 8000..=192000 Hz"
        ));
    }
    Ok(AudioFormat {
        sample_rate,
        channels,
    })
}

fn seek_sample_offset(
    target: &mut Option<Timestamp>,
    packet_timestamp: Timestamp,
    time_base: Option<TimeBase>,
    format: AudioFormat,
    sample_count: usize,
) -> Result<usize, String> {
    let available_frames = sample_count / usize::from(format.channels);
    let requested_skip = target
        .map(|target| {
            frames_before_timestamp(packet_timestamp, target, time_base, format.sample_rate)
        })
        .transpose()?
        .unwrap_or(0);
    let available_frames_u64 = u64::try_from(available_frames).unwrap_or(u64::MAX);
    let skip_frames = usize::try_from(requested_skip.min(available_frames_u64))
        .map_err(|_| "seek discard count does not fit memory bounds".to_owned())?;
    if requested_skip < available_frames_u64 {
        *target = None;
    }
    skip_frames
        .checked_mul(usize::from(format.channels))
        .ok_or_else(|| "seek discard sample count overflow".to_owned())
}

fn seek_source(
    format: &mut dyn FormatReader,
    track_id: u32,
    position: Duration,
) -> Result<SeekedTo, String> {
    format
        .seek(
            SeekMode::Accurate,
            SeekTo::Time {
                time: MediaTime::from_micros_u64(duration_micros(position)),
                track_id: Some(track_id),
            },
        )
        .map_err(|error| format!("cannot seek audio source: {error}"))
}

fn frames_before_timestamp(
    packet_timestamp: Timestamp,
    target: Timestamp,
    time_base: Option<TimeBase>,
    sample_rate: u32,
) -> Result<u64, String> {
    let Some(ticks) = target
        .get()
        .checked_sub(packet_timestamp.get())
        .filter(|ticks| *ticks > 0)
    else {
        return Ok(0);
    };
    let ticks = u128::try_from(ticks).map_err(|_| "seek timestamp is negative".to_owned())?;
    let time_base = time_base.ok_or_else(|| "audio track has no seek time base".to_owned())?;
    let numerator = ticks
        .checked_mul(u128::from(time_base.numer.get()))
        .and_then(|value| value.checked_mul(u128::from(sample_rate)))
        .ok_or_else(|| "seek discard frame count overflow".to_owned())?;
    let denominator = u128::from(time_base.denom.get());
    let frames = numerator
        .checked_add(denominator.saturating_sub(1))
        .ok_or_else(|| "seek discard frame count overflow".to_owned())?
        / denominator;
    u64::try_from(frames).map_err(|_| "seek discard frame count is too large".to_owned())
}

fn track_duration(track: &symphonia::core::formats::Track) -> Result<Option<Duration>, String> {
    let calculated = track
        .time_base
        .zip(track.duration)
        .and_then(|(time_base, duration)| {
            i64::try_from(duration.get())
                .ok()
                .and_then(|value| time_base.calc_time(value.into()))
        })
        .and_then(|time| u64::try_from(time.as_micros()).ok())
        .map(Duration::from_micros)
        .or_else(|| {
            let sample_rate = track
                .codec_params
                .as_ref()
                .and_then(|params| params.audio())
                .and_then(|params| params.sample_rate)?;
            let frames = track.num_frames?;
            let micros = u128::from(frames)
                .checked_mul(1_000_000)?
                .checked_div(u128::from(sample_rate))?;
            u64::try_from(micros).ok().map(Duration::from_micros)
        });
    if calculated.is_some_and(|duration| duration > MAX_TRACK_DURATION) {
        Err("audio track duration exceeds one year".into())
    } else {
        Ok(calculated)
    }
}

fn open_format<S: MediaSource + 'static>(source: S) -> Result<Box<dyn FormatReader>, String> {
    let source = PlaybackSource::new(source)?;
    let container = source.container;
    let stream = MediaSourceStream::new(Box::new(source), MediaSourceStreamOptions::default());
    let options = FormatOptions::default();
    let format: Box<dyn FormatReader> = match container {
        AudioContainer::Flac => Box::new(
            symphonia::default::formats::FlacReader::try_new(stream, options)
                .map_err(|error| format!("cannot open FLAC audio: {error}"))?,
        ),
        AudioContainer::Mp3 => Box::new(
            symphonia::default::formats::MpaReader::try_new(stream, options)
                .map_err(|error| format!("cannot open MPEG Layer III audio: {error}"))?,
        ),
        AudioContainer::Ogg => Box::new(
            symphonia::default::formats::OggReader::try_new(stream, options)
                .map_err(|error| format!("cannot open Ogg audio: {error}"))?,
        ),
        AudioContainer::Wav => Box::new(
            symphonia::default::formats::WavReader::try_new(stream, options)
                .map_err(|error| format!("cannot open WAV audio: {error}"))?,
        ),
    };
    Ok(format)
}

struct ProtocolSink<W: Write> {
    writer: W,
    ready: bool,
    scratch: Vec<u8>,
}

impl<W: Write> ProtocolSink<W> {
    fn new(writer: W) -> Self {
        Self {
            writer,
            ready: false,
            scratch: Vec::new(),
        }
    }

    fn finish(&mut self) -> Result<(), String> {
        if !self.ready {
            return Err("decoder completed without a format header".into());
        }
        self.writer
            .write_all(&0_u32.to_le_bytes())
            .and_then(|()| self.writer.flush())
            .map_err(|error| format!("cannot finish decoder protocol: {error}"))
    }

    fn error(&mut self, message: &str) -> Result<(), String> {
        let message = bounded_error(message);
        if self.ready {
            self.writer
                .write_all(&ERROR_FRAME.to_le_bytes())
                .map_err(|error| error.to_string())?;
        } else {
            self.writer
                .write_all(ERROR_MAGIC)
                .map_err(|error| error.to_string())?;
        }
        let length = u16::try_from(message.len()).expect("bounded decoder error fits u16");
        self.writer
            .write_all(&length.to_le_bytes())
            .and_then(|()| self.writer.write_all(message.as_bytes()))
            .and_then(|()| self.writer.flush())
            .map_err(|error| format!("cannot write decoder error: {error}"))
    }
}

impl<W: Write> PcmSink for ProtocolSink<W> {
    fn ready(&mut self, info: DecodedInfo) -> Result<(), String> {
        if self.ready {
            return Err("decoder attempted to send two format headers".into());
        }
        self.writer
            .write_all(READY_MAGIC)
            .and_then(|()| {
                self.writer
                    .write_all(&info.format.sample_rate.to_le_bytes())
            })
            .and_then(|()| self.writer.write_all(&info.format.channels.to_le_bytes()))
            .and_then(|()| self.writer.write_all(&[0, 0]))
            .and_then(|()| {
                let duration = info
                    .duration
                    .map_or(UNKNOWN_DURATION_MICROS, duration_micros);
                self.writer.write_all(&duration.to_le_bytes())
            })
            .and_then(|()| {
                let position = duration_micros(info.position);
                self.writer.write_all(&position.to_le_bytes())
            })
            .map_err(|error| format!("cannot write decoder format: {error}"))?;
        self.ready = true;
        Ok(())
    }

    fn samples(&mut self, samples: &[f32]) -> Result<(), String> {
        if !self.ready || samples.is_empty() || samples.len() > MAX_PCM_BLOCK_SAMPLES {
            return Err("decoder attempted to write an invalid PCM block".into());
        }
        let length = u32::try_from(samples.len()).expect("bounded PCM block fits u32");
        self.writer
            .write_all(&length.to_le_bytes())
            .map_err(|error| error.to_string())?;
        self.scratch.clear();
        self.scratch
            .try_reserve_exact(std::mem::size_of_val(samples))
            .map_err(|error| format!("cannot reserve PCM protocol scratch: {error}"))?;
        for sample in samples {
            self.scratch.extend_from_slice(&sample.to_le_bytes());
        }
        self.writer
            .write_all(&self.scratch)
            .map_err(|error| format!("cannot write decoder PCM: {error}"))
    }
}

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn bounded_error(message: &str) -> String {
    let mut bounded = String::new();
    for character in message.chars() {
        if bounded.len().saturating_add(character.len_utf8()) > MAX_ERROR_BYTES {
            break;
        }
        bounded.push(character);
    }
    if bounded.is_empty() {
        "decoder failed".into()
    } else {
        bounded
    }
}

/// Exercises the decoder adapter under the fuzzer's process and output bounds.
pub fn fuzz_decode(input: &[u8]) {
    struct CountingSink;
    impl PcmSink for CountingSink {
        fn ready(&mut self, _info: DecodedInfo) -> Result<(), String> {
            Ok(())
        }

        fn samples(&mut self, _samples: &[f32]) -> Result<(), String> {
            Ok(())
        }
    }

    let source = Cursor::new(input.to_vec());
    let _ = decode_source(
        source,
        &mut CountingSink,
        Some(FUZZ_SAMPLE_LIMIT),
        Duration::ZERO,
    );
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs::File;
    use std::io::Cursor;
    use std::process::Command;
    use std::time::Duration;

    use super::{
        AudioFormat, DecodedInfo, DecoderPoll, DecoderStream, HelperDecoder, PcmSink, decode_source,
    };

    #[derive(Default)]
    struct CollectSink {
        format: Option<AudioFormat>,
        duration: Option<Duration>,
        position: Duration,
        samples: Vec<f32>,
    }

    impl PcmSink for CollectSink {
        fn ready(&mut self, info: DecodedInfo) -> Result<(), String> {
            self.format = Some(info.format);
            self.duration = info.duration;
            self.position = info.position;
            Ok(())
        }

        fn samples(&mut self, samples: &[f32]) -> Result<(), String> {
            assert!(samples.iter().all(|sample| sample.is_finite()));
            self.samples.extend_from_slice(samples);
            Ok(())
        }
    }

    #[test]
    fn malformed_wav_channel_counts_return_errors_without_panicking() {
        let bytes = include_bytes!("../../tests/fixtures/audio/invalid-wav-channels.bin");
        let mut sink = CollectSink::default();
        let error = decode_source(Cursor::new(bytes.to_vec()), &mut sink, None, Duration::ZERO)
            .expect_err("unsupported WAV channel count");
        assert!(error.contains("channel count"), "{error}");
        assert!(sink.samples.is_empty());
    }

    #[test]
    fn wav_channel_validation_checks_later_format_chunks() {
        let good = include_bytes!("../../tests/fixtures/audio/tone.wav");
        let mut bytes =
            include_bytes!("../../tests/fixtures/audio/invalid-wav-channels.bin").to_vec();
        bytes.splice(12..12, good[12..36].iter().copied());
        let error = decode_source(
            Cursor::new(bytes),
            &mut CollectSink::default(),
            None,
            Duration::ZERO,
        )
        .expect_err("a later format chunk must also be checked");
        assert!(error.contains("channel count"), "{error}");
    }

    #[test]
    fn wav_header_padding_preserves_pcm() {
        let original = include_bytes!("../../tests/fixtures/audio/tone.wav");
        let mut expected = CollectSink::default();
        decode_source(
            Cursor::new(original.to_vec()),
            &mut expected,
            None,
            Duration::ZERO,
        )
        .expect("original WAV");
        let mut padded = original.to_vec();
        padded.splice(12..12, *b"JUNK\x01\0\0\0x\0");
        let length = u32::try_from(padded.len() - 8).expect("small fixture");
        padded[4..8].copy_from_slice(&length.to_le_bytes());
        let mut actual = CollectSink::default();
        decode_source(Cursor::new(padded), &mut actual, None, Duration::ZERO)
            .expect("odd header chunk");
        assert_eq!(actual.samples, expected.samples);
    }

    #[test]
    fn excessive_wav_header_chunks_are_rejected() {
        let original = include_bytes!("../../tests/fixtures/audio/tone.wav");
        let mut excessive = original.to_vec();
        excessive.splice(12..12, b"JUNK\0\0\0\0".repeat(super::MAX_WAV_HEADER_CHUNKS));
        let length = u32::try_from(excessive.len() - 8).expect("bounded fixture");
        excessive[4..8].copy_from_slice(&length.to_le_bytes());
        let error = decode_source(
            Cursor::new(excessive),
            &mut CollectSink::default(),
            None,
            Duration::ZERO,
        )
        .expect_err("header work limit");
        assert!(error.contains("WAV header exceeded"), "{error}");
    }

    #[test]
    fn compatibility_fixtures_decode_through_the_selected_pipeline() {
        let fixtures: BTreeMap<&str, &[u8]> = BTreeMap::from([
            (
                "FLAC / FLAC",
                include_bytes!("../../tests/fixtures/audio/tone.flac").as_slice(),
            ),
            (
                "MP3 / MPEG Layer III",
                include_bytes!("../../tests/fixtures/audio/tone.mp3").as_slice(),
            ),
            (
                "Ogg / Vorbis",
                include_bytes!("../../tests/fixtures/audio/tone.ogg").as_slice(),
            ),
            (
                "WAV / signed 16-bit PCM",
                include_bytes!("../../tests/fixtures/audio/tone.wav").as_slice(),
            ),
        ]);

        for (combination, bytes) in fixtures {
            let mut sink = CollectSink::default();
            decode_source(Cursor::new(bytes.to_vec()), &mut sink, None, Duration::ZERO)
                .unwrap_or_else(|error| panic!("{combination} failed: {error}"));
            assert_eq!(
                sink.format,
                Some(AudioFormat {
                    sample_rate: 48_000,
                    channels: 2
                }),
                "{combination} format"
            );
            assert!(
                (20_000..=30_000).contains(&sink.samples.len()),
                "{combination} emitted {} samples",
                sink.samples.len()
            );
            assert!(
                sink.duration
                    .is_some_and(|duration| duration > Duration::ZERO)
            );
            assert_eq!(sink.position, Duration::ZERO);
        }
    }

    #[test]
    fn a_track_below_the_usual_graph_rate_decodes_at_its_own_rate() {
        // Every other fixture is 48 kHz, which hid the output stage assuming the
        // device would advertise the track's rate. Keep one 44.1 kHz track here so
        // rate handling is never again exercised only at the common graph rate.
        let bytes = include_bytes!("../../tests/fixtures/audio/tone-44100.wav").as_slice();
        let mut sink = CollectSink::default();

        decode_source(Cursor::new(bytes.to_vec()), &mut sink, None, Duration::ZERO)
            .expect("decode the 44.1 kHz fixture");

        assert_eq!(
            sink.format,
            Some(AudioFormat {
                sample_rate: 44_100,
                channels: 2
            })
        );
        assert!(
            (20_000..=25_000).contains(&sink.samples.len()),
            "emitted {} samples",
            sink.samples.len()
        );
        assert!(
            sink.duration
                .is_some_and(|duration| duration > Duration::ZERO)
        );
    }

    #[test]
    fn accurate_seek_discards_preroll_for_every_verified_container() {
        let fixtures: BTreeMap<&str, &[u8]> = BTreeMap::from([
            (
                "FLAC",
                include_bytes!("../../tests/fixtures/audio/tone.flac").as_slice(),
            ),
            (
                "MP3",
                include_bytes!("../../tests/fixtures/audio/tone.mp3").as_slice(),
            ),
            (
                "Ogg",
                include_bytes!("../../tests/fixtures/audio/tone.ogg").as_slice(),
            ),
            (
                "WAV",
                include_bytes!("../../tests/fixtures/audio/tone.wav").as_slice(),
            ),
        ]);
        let requested = Duration::from_millis(100);

        for (container, bytes) in fixtures {
            let mut full = CollectSink::default();
            decode_source(Cursor::new(bytes.to_vec()), &mut full, None, Duration::ZERO)
                .unwrap_or_else(|error| panic!("decode complete {container} fixture: {error}"));
            let mut sought = CollectSink::default();
            decode_source(Cursor::new(bytes.to_vec()), &mut sought, None, requested)
                .unwrap_or_else(|error| panic!("seek within {container} fixture: {error}"));

            assert_eq!(sought.position, requested, "{container}");
            assert_eq!(sought.duration, full.duration, "{container}");
            let requested_samples = 4_800 * 2;
            assert_eq!(
                sought.samples.len(),
                full.samples.len().saturating_sub(requested_samples),
                "{container} sample count"
            );
            if container == "WAV" {
                assert_eq!(
                    &sought.samples[..64],
                    &full.samples[requested_samples..requested_samples + 64],
                    "WAV first emitted frame must be the requested frame"
                );
            }
        }
    }

    #[test]
    fn malformed_input_is_refused_without_announcing_pcm() {
        let mut sink = CollectSink::default();

        let error = decode_source(
            Cursor::new(vec![0_u8; 4_096]),
            &mut sink,
            None,
            Duration::ZERO,
        )
        .expect_err("malformed media must be refused");

        assert!(error.contains("supported container"), "{error}");
        assert_eq!(sink.format, None);
        assert!(sink.samples.is_empty());
    }

    #[test]
    fn malformed_flac_picture_length_is_refused_before_decoder_allocation() {
        // Minimized from the audio fuzzer: Symphonia otherwise treats bytes inside the
        // truncated picture block as an allocation length of almost four GiB.
        let reproducer = b"fLaC\x06fLaC\x06\x00\xff\xff\xff\x00\xff\xff\xff\xff";
        let mut sink = CollectSink::default();

        let error = decode_source(
            Cursor::new(reproducer.to_vec()),
            &mut sink,
            None,
            Duration::ZERO,
        )
        .expect_err("truncated FLAC metadata must be refused");

        assert!(error.contains("FLAC metadata header"), "{error}");
        assert_eq!(sink.format, None);
        assert!(sink.samples.is_empty());
    }

    #[test]
    fn uncooperative_decoder_helper_is_killed_and_reaped() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "while :; do :; done"]);
        let file = File::open("/dev/null").expect("open harmless input");
        let mut decoder = HelperDecoder::start_with(command, &file, Duration::from_millis(20))
            .expect("start uncooperative helper");
        let message = decoder
            .messages
            .as_ref()
            .expect("decoder messages")
            .recv_timeout(Duration::from_secs(1))
            .expect("bounded no-progress report");

        assert!(matches!(
            message,
            DecoderPoll::Failed(message) if message.contains("made no output progress")
        ));
        decoder.shutdown().expect("kill and reap helper");
    }
}
