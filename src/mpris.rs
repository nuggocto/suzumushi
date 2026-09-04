// SPDX-License-Identifier: Apache-2.0

//! Bounded MPRIS bridge for the active terminal session.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use zbus::connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::app::{AppState, PlaybackStatus, RepeatMode};
use crate::errors::{AppError, AppResult};
use crate::input::AppAction;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.suzumushi";
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";
const TRACK_PATH_PREFIX: &str = "/io/github/nuggocto/suzumushi/track/";
const NO_TRACK_PATH: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";
pub(crate) const REQUEST_CAPACITY: usize = 32;
const BUS_MESSAGE_CAPACITY: usize = 8;
const WORKER_POLL: Duration = Duration::from_millis(25);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const TEXT_MAX_BYTES: usize = 1_024;
const CAN_GO_NEXT: u8 = 1 << 0;
const CAN_GO_PREVIOUS: u8 = 1 << 1;
const CAN_PLAY: u8 = 1 << 2;
const CAN_PAUSE: u8 = 1 << 3;
const CAN_SEEK: u8 = 1 << 4;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MprisProjection {
    playback_status: PlaybackStatus,
    repeat: RepeatMode,
    shuffle: bool,
    volume_percent: u8,
    title: String,
    creator: String,
    track_token: Option<(u64, u64)>,
    position: Duration,
    duration: Option<Duration>,
    capabilities: u8,
    seek_revision: u64,
}

impl MprisProjection {
    #[must_use]
    pub(crate) fn from_app(app: &AppState) -> Self {
        let track_token = app.current_track_token();
        let (title, creator) = if track_token.is_some() {
            let (title, creator) = app.state_identity(TEXT_MAX_BYTES);
            (
                mpris_text(&title, TEXT_MAX_BYTES),
                mpris_text(&creator, TEXT_MAX_BYTES),
            )
        } else {
            (String::new(), String::new())
        };
        let active = matches!(
            app.playback_status,
            PlaybackStatus::Playing | PlaybackStatus::Paused
        );
        let mut capabilities = 0;
        capabilities |= u8::from(app.can_go_next()) * CAN_GO_NEXT;
        capabilities |= u8::from(app.current_track_token().is_some()) * CAN_GO_PREVIOUS;
        capabilities |= u8::from(!app.queue.is_empty()) * CAN_PLAY;
        capabilities |= u8::from(track_token.is_some()) * CAN_PAUSE;
        capabilities |= u8::from(active) * CAN_SEEK;
        Self {
            playback_status: app.playback_status,
            repeat: app.repeat,
            shuffle: app.shuffle,
            volume_percent: app.volume_percent,
            title,
            creator,
            track_token,
            position: app.playback_position(),
            duration: app.playback_duration(),
            capabilities,
            seek_revision: app.seek_revision(),
        }
    }
}

pub(crate) struct MprisRuntime {
    actions: Receiver<AppAction>,
    latest: Arc<Mutex<Option<MprisProjection>>>,
    shutdown: Option<SyncSender<()>>,
    worker: Option<JoinHandle<()>>,
}

impl MprisRuntime {
    /// Starts the single session-bus identity and its owned worker.
    pub(crate) fn start(initial: MprisProjection) -> AppResult<Self> {
        let (action_sender, actions) = mpsc::sync_channel(REQUEST_CAPACITY);
        let latest = Arc::new(Mutex::new(None));
        let worker_latest = Arc::clone(&latest);
        let (shutdown, shutdown_receiver) = mpsc::sync_channel(1);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("suzumushi-mpris".into())
            .spawn(move || {
                worker_main(
                    initial,
                    action_sender,
                    &worker_latest,
                    &shutdown_receiver,
                    &ready_sender,
                );
            })
            .map_err(|error| AppError::Mpris(format!("cannot start worker: {error}")))?;
        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                actions,
                latest,
                shutdown: Some(shutdown),
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(AppError::Mpris(error))
            }
            Err(_) => {
                let _ = worker.join();
                Err(AppError::Mpris("worker stopped during startup".into()))
            }
        }
    }

    pub(crate) fn publish(&self, projection: MprisProjection) -> AppResult<()> {
        let mut latest = self
            .latest
            .lock()
            .map_err(|_| AppError::Mpris("projection lane was poisoned".into()))?;
        *latest = Some(projection);
        Ok(())
    }

    pub(crate) fn try_action(&self) -> Option<AppAction> {
        self.actions.try_recv().ok()
    }

    pub(crate) fn shutdown(mut self) -> AppResult<()> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> AppResult<()> {
        if let Some(sender) = self.shutdown.take() {
            let _ = sender.send(());
        }
        let Some(worker) = self.worker.take() else {
            return Ok(());
        };
        worker
            .join()
            .map_err(|_| AppError::Mpris("worker panicked during shutdown".into()))
    }
}

impl Drop for MprisRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

fn worker_main(
    initial: MprisProjection,
    actions: SyncSender<AppAction>,
    latest: &Arc<Mutex<Option<MprisProjection>>>,
    shutdown: &Receiver<()>,
    ready: &SyncSender<Result<(), String>>,
) {
    let state = Arc::new(RwLock::new(initial));
    let root = RootInterface {
        actions: actions.clone(),
    };
    let player = PlayerInterface {
        actions,
        state: Arc::clone(&state),
    };
    let built = connection::Builder::session()
        .and_then(|builder| builder.max_queued(BUS_MESSAGE_CAPACITY).name(BUS_NAME))
        .and_then(|builder| builder.serve_at(OBJECT_PATH, root))
        .and_then(|builder| builder.serve_at(OBJECT_PATH, player))
        .map_err(|error| error.to_string())
        .and_then(|builder| connect_with_timeout(builder, CONNECT_TIMEOUT));
    let connection = match built {
        Ok(connection) => connection,
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    if ready.send(Ok(())).is_err() {
        drop(connection);
        return;
    }

    loop {
        match shutdown.recv_timeout(WORKER_POLL) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let next = latest.lock().ok().and_then(|mut slot| slot.take());
        let Some(next) = next else {
            continue;
        };
        let previous = match state.write() {
            Ok(mut current) => std::mem::replace(&mut *current, next.clone()),
            Err(_) => break,
        };
        if let Err(error) = emit_changes(&connection, &previous, &next) {
            tracing::warn!(%error, "MPRIS property update failed");
        }
    }
    if let Some(next) = latest.lock().ok().and_then(|mut slot| slot.take())
        && let Ok(mut current) = state.write()
    {
        let previous = std::mem::replace(&mut *current, next.clone());
        let _ = emit_changes(&connection, &previous, &next);
    }
    drop(connection);
}

fn connect_with_timeout(
    builder: connection::Builder<'_>,
    timeout: Duration,
) -> Result<zbus::blocking::Connection, String> {
    // Dropping the losing build future closes its socket and cancels setup.
    // The worker then returns its startup error and can be joined normally.
    async_io::block_on(futures_lite::future::or(
        async { builder.build().await.map_err(|error| error.to_string()) },
        async {
            async_io::Timer::after(timeout).await;
            Err("session bus connection timed out".into())
        },
    ))
    .map(zbus::blocking::Connection::from)
}

struct RootInterface {
    actions: SyncSender<AppAction>,
}

#[allow(clippy::unused_self)]
#[zbus::interface(name = "org.mpris.MediaPlayer2", introspection_docs = false)]
impl RootInterface {
    fn raise(&self) {}

    fn quit(&self) -> zbus::fdo::Result<()> {
        enqueue(&self.actions, AppAction::Quit)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn can_quit(&self) -> bool {
        true
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn can_raise(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn has_track_list(&self) -> bool {
        false
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn identity(&self) -> &'static str {
        "Suzumushi"
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn desktop_entry(&self) -> &'static str {
        ""
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn fullscreen(&self) -> bool {
        false
    }

    #[zbus(property)]
    const fn set_fullscreen(&self, fullscreen: bool) {
        let _ = fullscreen;
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn can_set_fullscreen(&self) -> bool {
        false
    }
}

struct PlayerInterface {
    actions: SyncSender<AppAction>,
    state: Arc<RwLock<MprisProjection>>,
}

impl PlayerInterface {
    fn with_state<T>(&self, read: impl FnOnce(&MprisProjection) -> T) -> zbus::fdo::Result<T> {
        self.state
            .read()
            .map(|state| read(&state))
            .map_err(|_| zbus::fdo::Error::Failed("player projection unavailable".into()))
    }

    fn enqueue_if(
        &self,
        allowed: impl FnOnce(&MprisProjection) -> bool,
        action: AppAction,
    ) -> zbus::fdo::Result<()> {
        if self.with_state(allowed)? {
            enqueue(&self.actions, action)
        } else {
            Ok(())
        }
    }
}

#[allow(clippy::unused_self)]
#[zbus::interface(name = "org.mpris.MediaPlayer2.Player", introspection_docs = false)]
impl PlayerInterface {
    fn next(&self) -> zbus::fdo::Result<()> {
        self.enqueue_if(|state| has_capability(state, CAN_GO_NEXT), AppAction::Next)
    }

    fn previous(&self) -> zbus::fdo::Result<()> {
        self.enqueue_if(
            |state| has_capability(state, CAN_GO_PREVIOUS),
            AppAction::Previous,
        )
    }

    fn pause(&self) -> zbus::fdo::Result<()> {
        self.enqueue_if(|state| has_capability(state, CAN_PAUSE), AppAction::Pause)
    }

    fn play_pause(&self) -> zbus::fdo::Result<()> {
        self.enqueue_if(
            |state| has_capability(state, CAN_PLAY),
            AppAction::PlayPause,
        )
    }

    fn stop(&self) -> zbus::fdo::Result<()> {
        enqueue(&self.actions, AppAction::Stop)
    }

    fn play(&self) -> zbus::fdo::Result<()> {
        self.enqueue_if(|state| has_capability(state, CAN_PLAY), AppAction::Play)
    }

    fn seek(&self, offset: i64) -> zbus::fdo::Result<()> {
        if offset == 0 {
            return Ok(());
        }
        self.enqueue_if(
            |state| has_capability(state, CAN_SEEK),
            AppAction::SeekRelative {
                forward: offset.is_positive(),
                distance: Duration::from_micros(offset.unsigned_abs()),
            },
        )
    }

    #[allow(clippy::needless_pass_by_value)]
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) -> zbus::fdo::Result<()> {
        let Some((playback_generation, queue_instance)) = parse_track_path(&track_id) else {
            return Ok(());
        };
        let Ok(position) = u64::try_from(position) else {
            return Ok(());
        };
        enqueue(
            &self.actions,
            AppAction::SeekAbsolute {
                playback_generation,
                queue_instance,
                position: Duration::from_micros(position),
            },
        )
    }

    fn open_uri(&self, uri: &str) -> zbus::fdo::Result<()> {
        let _ = uri;
        Err(zbus::fdo::Error::NotSupported(
            "Suzumushi plays only its scanned local library".into(),
        ))
    }

    #[zbus(property)]
    fn playback_status(&self) -> zbus::fdo::Result<&'static str> {
        self.with_state(|state| playback_status_label(state.playback_status))
    }

    #[zbus(property)]
    fn loop_status(&self) -> zbus::fdo::Result<&'static str> {
        self.with_state(|state| loop_status_label(state.repeat))
    }

    #[zbus(property)]
    fn set_loop_status(&self, value: &str) -> zbus::fdo::Result<()> {
        let action = match value {
            "None" => AppAction::RepeatOff,
            "Playlist" => AppAction::RepeatAll,
            "Track" => AppAction::RepeatOne,
            _ => {
                return Err(zbus::fdo::Error::InvalidArgs(
                    "LoopStatus must be None, Playlist, or Track".into(),
                ));
            }
        };
        enqueue(&self.actions, action)
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn set_rate(&self, value: f64) -> zbus::fdo::Result<()> {
        if value.to_bits() == 1.0_f64.to_bits() {
            Ok(())
        } else {
            Err(zbus::fdo::Error::InvalidArgs(
                "Suzumushi supports only 1.0x playback".into(),
            ))
        }
    }

    #[zbus(property)]
    fn shuffle(&self) -> zbus::fdo::Result<bool> {
        self.with_state(|state| state.shuffle)
    }

    #[zbus(property)]
    fn set_shuffle(&self, enabled: bool) -> zbus::fdo::Result<()> {
        enqueue(&self.actions, AppAction::SetShuffle(enabled))
    }

    #[zbus(property)]
    fn metadata(&self) -> zbus::fdo::Result<HashMap<String, OwnedValue>> {
        self.with_state(metadata_from_projection)
    }

    #[zbus(property)]
    fn volume(&self) -> zbus::fdo::Result<f64> {
        self.with_state(|state| f64::from(state.volume_percent) / 100.0)
    }

    #[zbus(property)]
    fn set_volume(&self, volume: f64) -> zbus::fdo::Result<()> {
        if !volume.is_finite() {
            return Err(zbus::fdo::Error::InvalidArgs(
                "Volume must be a finite number".into(),
            ));
        }
        let scaled = volume.clamp(0.0, 1.0) * 100.0;
        let mut percent = 0_u8;
        while percent < 100 && f64::from(percent) + 0.5 <= scaled {
            percent += 1;
        }
        enqueue(&self.actions, AppAction::SetVolume(percent))
    }

    #[zbus(property(emits_changed_signal = "false"))]
    fn position(&self) -> zbus::fdo::Result<i64> {
        self.with_state(|state| duration_micros(state.position))
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn minimum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn maximum_rate(&self) -> f64 {
        1.0
    }

    #[zbus(property)]
    fn can_go_next(&self) -> zbus::fdo::Result<bool> {
        self.with_state(|state| has_capability(state, CAN_GO_NEXT))
    }

    #[zbus(property)]
    fn can_go_previous(&self) -> zbus::fdo::Result<bool> {
        self.with_state(|state| has_capability(state, CAN_GO_PREVIOUS))
    }

    #[zbus(property)]
    fn can_play(&self) -> zbus::fdo::Result<bool> {
        self.with_state(|state| has_capability(state, CAN_PLAY))
    }

    #[zbus(property)]
    fn can_pause(&self) -> zbus::fdo::Result<bool> {
        self.with_state(|state| has_capability(state, CAN_PAUSE))
    }

    #[zbus(property)]
    fn can_seek(&self) -> zbus::fdo::Result<bool> {
        self.with_state(|state| has_capability(state, CAN_SEEK))
    }

    #[zbus(property(emits_changed_signal = "const"))]
    const fn can_control(&self) -> bool {
        true
    }

    #[zbus(signal)]
    async fn seeked(emitter: &SignalEmitter<'_>, position: i64) -> zbus::Result<()>;
}

fn emit_changes(
    connection: &zbus::blocking::Connection,
    previous: &MprisProjection,
    current: &MprisProjection,
) -> zbus::Result<()> {
    let mut changed = HashMap::new();
    if previous.playback_status != current.playback_status {
        changed.insert(
            "PlaybackStatus",
            Value::from(playback_status_label(current.playback_status)),
        );
    }
    if previous.repeat != current.repeat {
        changed.insert("LoopStatus", Value::from(loop_status_label(current.repeat)));
    }
    if previous.shuffle != current.shuffle {
        changed.insert("Shuffle", Value::from(current.shuffle));
    }
    if previous.volume_percent != current.volume_percent {
        changed.insert(
            "Volume",
            Value::from(f64::from(current.volume_percent) / 100.0),
        );
    }
    if previous.track_token != current.track_token
        || previous.title != current.title
        || previous.creator != current.creator
        || previous.duration != current.duration
    {
        changed.insert("Metadata", Value::from(metadata_from_projection(current)));
    }
    if has_capability(previous, CAN_GO_NEXT) != has_capability(current, CAN_GO_NEXT) {
        changed.insert(
            "CanGoNext",
            Value::from(has_capability(current, CAN_GO_NEXT)),
        );
    }
    if has_capability(previous, CAN_GO_PREVIOUS) != has_capability(current, CAN_GO_PREVIOUS) {
        changed.insert(
            "CanGoPrevious",
            Value::from(has_capability(current, CAN_GO_PREVIOUS)),
        );
    }
    if has_capability(previous, CAN_PLAY) != has_capability(current, CAN_PLAY) {
        changed.insert("CanPlay", Value::from(has_capability(current, CAN_PLAY)));
    }
    if has_capability(previous, CAN_PAUSE) != has_capability(current, CAN_PAUSE) {
        changed.insert("CanPause", Value::from(has_capability(current, CAN_PAUSE)));
    }
    if has_capability(previous, CAN_SEEK) != has_capability(current, CAN_SEEK) {
        changed.insert("CanSeek", Value::from(has_capability(current, CAN_SEEK)));
    }
    if !changed.is_empty() {
        connection.emit_signal(
            None::<&str>,
            OBJECT_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            &(PLAYER_INTERFACE, changed, Vec::<&str>::new()),
        )?;
    }
    if previous.track_token == current.track_token
        && previous.seek_revision != current.seek_revision
    {
        connection.emit_signal(
            None::<&str>,
            OBJECT_PATH,
            PLAYER_INTERFACE,
            "Seeked",
            &duration_micros(current.position),
        )?;
    }
    Ok(())
}

const fn playback_status_label(status: PlaybackStatus) -> &'static str {
    match status {
        PlaybackStatus::Playing => "Playing",
        PlaybackStatus::Paused => "Paused",
        PlaybackStatus::Stopped | PlaybackStatus::Loading | PlaybackStatus::Error => "Stopped",
    }
}

const fn loop_status_label(repeat: RepeatMode) -> &'static str {
    match repeat {
        RepeatMode::Off => "None",
        RepeatMode::All => "Playlist",
        RepeatMode::One => "Track",
    }
}

fn enqueue(sender: &SyncSender<AppAction>, action: AppAction) -> zbus::fdo::Result<()> {
    match sender.try_send(action) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(zbus::fdo::Error::LimitsExceeded(
            "desktop-control queue is full".into(),
        )),
        Err(TrySendError::Disconnected(_)) => Err(zbus::fdo::Error::Failed(
            "terminal session is stopping".into(),
        )),
    }
}

const fn has_capability(projection: &MprisProjection, capability: u8) -> bool {
    projection.capabilities & capability != 0
}

fn metadata_from_projection(projection: &MprisProjection) -> HashMap<String, OwnedValue> {
    let mut metadata = HashMap::new();
    let track_path = projection
        .track_token
        .map_or_else(|| NO_TRACK_PATH.to_owned(), track_path);
    if let Ok(path) = ObjectPath::try_from(track_path) {
        metadata.insert("mpris:trackid".into(), OwnedValue::from(path));
    }
    let Some(_) = projection.track_token else {
        return metadata;
    };
    if let Ok(title) = OwnedValue::try_from(Value::from(projection.title.clone())) {
        metadata.insert("xesam:title".into(), title);
    }
    if !projection.creator.is_empty()
        && let Ok(artist) = OwnedValue::try_from(Value::from(vec![projection.creator.clone()]))
    {
        metadata.insert("xesam:artist".into(), artist);
    }
    if let Some(duration) = projection.duration {
        metadata.insert("mpris:length".into(), duration_micros(duration).into());
    }
    metadata
}

fn track_path((playback_generation, queue_instance): (u64, u64)) -> String {
    format!("{TRACK_PATH_PREFIX}g{playback_generation}/q{queue_instance}")
}

fn parse_track_path(path: &ObjectPath<'_>) -> Option<(u64, u64)> {
    let suffix = path.as_str().strip_prefix(TRACK_PATH_PREFIX)?;
    let (generation, instance) = suffix.split_once('/')?;
    if generation.is_empty()
        || instance.is_empty()
        || !generation.starts_with('g')
        || !instance.starts_with('q')
        || generation[1..].is_empty()
        || instance[1..].is_empty()
        || generation[1..].starts_with('0')
        || instance[1..].starts_with('0')
        || !generation[1..].bytes().all(|byte| byte.is_ascii_digit())
        || !instance[1..].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some((generation[1..].parse().ok()?, instance[1..].parse().ok()?))
}

fn duration_micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

fn mpris_text(input: &str, max_bytes: usize) -> String {
    let mut output = String::new();
    let _ = output.try_reserve(input.len().min(max_bytes));
    for character in input.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if output.len().saturating_add(character.len_utf8()) > max_bytes {
            break;
        }
        output.push(character);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::sync::{Arc, RwLock, mpsc};
    use std::time::Duration;

    use zbus::zvariant::{ObjectPath, OwnedObjectPath};

    use super::{
        CAN_GO_NEXT, CAN_GO_PREVIOUS, CAN_PAUSE, CAN_PLAY, CAN_SEEK, MprisProjection,
        PlayerInterface, REQUEST_CAPACITY, TEXT_MAX_BYTES, metadata_from_projection, mpris_text,
        parse_track_path, track_path,
    };
    use crate::app::{PlaybackStatus, RepeatMode};
    use crate::input::AppAction;

    fn projection() -> MprisProjection {
        MprisProjection {
            playback_status: PlaybackStatus::Playing,
            repeat: RepeatMode::Off,
            shuffle: false,
            volume_percent: 80,
            title: "Title".into(),
            creator: "Artist".into(),
            track_token: Some((7, 11)),
            position: Duration::from_secs(3),
            duration: Some(Duration::from_mins(1)),
            capabilities: CAN_GO_NEXT | CAN_GO_PREVIOUS | CAN_PLAY | CAN_PAUSE | CAN_SEEK,
            seek_revision: 0,
        }
    }

    #[test]
    fn stalled_bus_authentication_times_out_and_closes_the_connection() {
        let (client, mut server) = UnixStream::pair().expect("isolated bus socket");
        server
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("test deadline");
        let worker = std::thread::spawn(move || {
            super::connect_with_timeout(
                zbus::connection::Builder::async_io_unix_stream(client),
                Duration::from_millis(100),
            )
        });
        let mut request = Vec::new();
        server
            .read_to_end(&mut request)
            .expect("cancelled setup closes its socket");
        assert!(
            request.starts_with(b"\0AUTH"),
            "real authentication was attempted"
        );
        assert_eq!(
            worker
                .join()
                .expect("connection worker")
                .expect_err("stalled bus"),
            "session bus connection timed out"
        );
    }

    #[test]
    fn metadata_is_bounded_sanitized_and_contains_no_artwork() {
        let input = format!("bad\0line\n{}", "é".repeat(TEXT_MAX_BYTES));
        let safe = mpris_text(&input, TEXT_MAX_BYTES);
        assert!(safe.len() <= TEXT_MAX_BYTES);
        assert!(!safe.chars().any(char::is_control));
        assert!(safe.starts_with("bad line "));

        let mut state = projection();
        state.title = safe;
        let metadata = metadata_from_projection(&state);
        assert!(metadata.contains_key("mpris:trackid"));
        assert!(metadata.contains_key("mpris:length"));
        assert!(metadata.contains_key("xesam:title"));
        assert!(metadata.contains_key("xesam:artist"));
        assert!(!metadata.contains_key("mpris:artUrl"));
    }

    #[test]
    fn track_paths_round_trip_and_reject_lookalikes() {
        let path = track_path((u64::MAX, 42));
        let path = ObjectPath::try_from(path).expect("valid owned track path");
        assert_eq!(parse_track_path(&path), Some((u64::MAX, 42)));
        for invalid in [
            "/io/github/nuggocto/suzumushi/track/g1/q2/extra",
            "/io/github/nuggocto/suzumushi/track/g1/q_2",
            "/io/github/nuggocto/suzumushi/track/g01/notq2",
            "/org/mpris/MediaPlayer2/TrackList/NoTrack",
        ] {
            let path = OwnedObjectPath::try_from(invalid).expect("valid object path syntax");
            assert_eq!(parse_track_path(&path), None, "{invalid}");
        }
    }

    #[test]
    fn method_arguments_become_bounded_app_actions() {
        let (sender, receiver) = mpsc::sync_channel(REQUEST_CAPACITY);
        let player = PlayerInterface {
            actions: sender,
            state: Arc::new(RwLock::new(projection())),
        };

        player.seek(i64::MIN).expect("minimum offset is bounded");
        assert_eq!(
            receiver.recv().expect("seek action"),
            AppAction::SeekRelative {
                forward: false,
                distance: Duration::from_micros(i64::MIN.unsigned_abs()),
            }
        );
        player.set_volume(0.376).expect("finite volume");
        assert_eq!(
            receiver.recv().expect("volume action"),
            AppAction::SetVolume(38)
        );
        assert!(player.set_volume(f64::NAN).is_err());

        let path = ObjectPath::try_from(track_path((7, 11))).expect("valid track path");
        player
            .set_position(path.clone(), 4_500_000)
            .expect("valid absolute position");
        assert_eq!(
            receiver.recv().expect("absolute seek action"),
            AppAction::SeekAbsolute {
                playback_generation: 7,
                queue_instance: 11,
                position: Duration::from_millis(4_500),
            }
        );
        player
            .set_position(path, -1)
            .expect("negative position is ignored");
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn saturated_request_lane_fails_without_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let player = PlayerInterface {
            actions: sender,
            state: Arc::new(RwLock::new(projection())),
        };
        player.play().expect("first request fits");
        assert!(player.play().is_err());
    }
}
