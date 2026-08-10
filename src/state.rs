// SPDX-License-Identifier: Apache-2.0

//! Bounded, descriptor-rooted projection of the app-owned playback state.

use std::fs::File;
use std::io::Write;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::fd::AsFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::getuid;
use serde::Serialize;

use crate::app::{AppState, PlaybackStatus};
use crate::errors::{AppError, AppResult};

const STATE_VERSION: u32 = 1;
const STATE_FILE: &str = "now-playing.json";
const STATE_MAX_BYTES: usize = 16 * 1_024;
// JSON control escapes can occupy six bytes, so two fields at this bound still
// leave room for the fixed document fields inside STATE_MAX_BYTES.
const STATE_IDENTITY_MAX_BYTES: usize = 1_024;

/// Stable app-owned fields consumed by later read-only status commands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct NowPlayingProjection {
    version: u32,
    playback_generation: u64,
    status: &'static str,
    title: String,
    creator: String,
    position_ms: u64,
    duration_ms: Option<u64>,
    volume_percent: u8,
    muted: bool,
    speed_percent: u16,
    shuffle: bool,
    repeat: &'static str,
    queue_position: Option<usize>,
    queue_length: usize,
}

impl NowPlayingProjection {
    #[must_use]
    pub(crate) fn from_app(app: &AppState) -> Self {
        let (title, creator) = app.state_identity(STATE_IDENTITY_MAX_BYTES);
        Self {
            version: STATE_VERSION,
            playback_generation: app.playback_generation,
            status: playback_status(app.playback_status),
            title,
            creator,
            position_ms: duration_millis(app.playback_position()),
            duration_ms: app.playback_duration().map(duration_millis),
            volume_percent: app.volume_percent,
            muted: app.muted,
            speed_percent: app.speed.percent(),
            shuffle: app.shuffle,
            repeat: app.repeat.label(),
            queue_position: app.queue_position(),
            queue_length: app.queue.len(),
        }
    }
}

#[derive(Serialize)]
struct StateDocument<'a> {
    #[serde(flatten)]
    projection: &'a NowPlayingProjection,
    updated_at_unix_ms: u64,
}

/// Owns the verified private state directory for one terminal session.
pub(crate) struct NowPlayingWriter {
    directory: File,
    directory_path: PathBuf,
    next_temporary: u64,
}

impl NowPlayingWriter {
    pub(crate) fn open(root: BorrowedFd<'_>, root_path: &Path) -> AppResult<Self> {
        let directory_path = root_path.join("state");
        let directory = rustix::fs::openat(
            root,
            "state",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| AppError::io("open state directory", &directory_path, error.into()))?;
        verify_private_directory(&directory, &directory_path)?;
        verify_existing_projection(&directory, &directory_path)?;
        Ok(Self {
            directory: File::from(directory),
            directory_path,
            next_temporary: 0,
        })
    }

    pub(crate) fn write(&mut self, projection: &NowPlayingProjection) -> AppResult<()> {
        let updated_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                AppError::Resource(format!("system clock precedes Unix epoch: {error}"))
            })?
            .as_millis();
        let updated_at_unix_ms = u64::try_from(updated_at_unix_ms).unwrap_or(u64::MAX);
        let mut bytes = serde_json::to_vec(&StateDocument {
            projection,
            updated_at_unix_ms,
        })
        .map_err(|error| AppError::Resource(format!("cannot encode playback state: {error}")))?;
        bytes.push(b'\n');
        if bytes.len() > STATE_MAX_BYTES {
            return Err(AppError::Resource(format!(
                "playback state exceeds the {STATE_MAX_BYTES}-byte limit"
            )));
        }

        let temporary = self.temporary_name()?;
        let temporary_path = self.directory_path.join(&temporary);
        let fd = rustix::fs::openat(
            &self.directory,
            &temporary,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(|error| {
            AppError::io(
                "create temporary playback state",
                &temporary_path,
                error.into(),
            )
        })?;
        let result = (|| {
            rustix::fs::fchmod(&fd, Mode::from_raw_mode(0o600)).map_err(|error| {
                AppError::io(
                    "set playback state permissions",
                    &temporary_path,
                    error.into(),
                )
            })?;
            // Readers need one complete snapshot; crash durability is unnecessary for ephemeral state.
            let mut file = File::from(fd);
            file.write_all(&bytes)
                .map_err(|error| AppError::io("write playback state", &temporary_path, error))?;
            rustix::fs::renameat(&self.directory, &temporary, &self.directory, STATE_FILE)
                .map_err(|error| {
                    AppError::io(
                        "replace playback state",
                        self.directory_path.join(STATE_FILE),
                        error.into(),
                    )
                })?;
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.directory, &temporary, AtFlags::empty());
        }
        result
    }

    fn temporary_name(&mut self) -> AppResult<String> {
        let counter = self.next_temporary;
        self.next_temporary = self.next_temporary.checked_add(1).ok_or_else(|| {
            AppError::Resource("playback state temporary counter exhausted".into())
        })?;
        Ok(format!(
            ".now-playing.{}.{}.tmp",
            std::process::id(),
            counter
        ))
    }
}

fn verify_private_directory(directory: impl AsFd, path: &Path) -> AppResult<()> {
    let stat = rustix::fs::fstat(directory)
        .map_err(|error| AppError::io("inspect state directory", path, error.into()))?;
    if !FileType::from_raw_mode(stat.st_mode).is_dir()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o700
    {
        return Err(AppError::InvalidRoot {
            path: path.to_path_buf(),
            reason: "state must be a current-user-owned directory with mode 0700".into(),
        });
    }
    Ok(())
}

fn verify_existing_projection(directory: impl AsFd, directory_path: &Path) -> AppResult<()> {
    let fd = match rustix::fs::openat(
        directory,
        STATE_FILE,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(()),
        Err(error) => {
            return Err(AppError::io(
                "open existing playback state",
                directory_path.join(STATE_FILE),
                error.into(),
            ));
        }
    };
    let stat = rustix::fs::fstat(&fd).map_err(|error| {
        AppError::io(
            "inspect existing playback state",
            directory_path.join(STATE_FILE),
            error.into(),
        )
    })?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(AppError::InvalidRoot {
            path: directory_path.join(STATE_FILE),
            reason: "playback state must be a current-user-owned regular file with mode 0600 and one link"
                .into(),
        });
    }
    Ok(())
}

const fn playback_status(status: PlaybackStatus) -> &'static str {
    match status {
        PlaybackStatus::Stopped => "stopped",
        PlaybackStatus::Loading => "loading",
        PlaybackStatus::Playing => "playing",
        PlaybackStatus::Paused => "paused",
        PlaybackStatus::Error => "error",
    }
}

fn duration_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    use rustix::fd::AsFd;

    use super::{
        NowPlayingProjection, NowPlayingWriter, STATE_IDENTITY_MAX_BYTES, STATE_MAX_BYTES,
    };
    use crate::app::AppState;
    use crate::config::Config;
    use crate::input::AppAction;
    use crate::model::{
        FileIdentity, MediaAsset, MediaAssetId, ScanCounters, ScanIndex, SearchFields, TrackEntry,
        TrackEntryId, TrackEntrySource, TrackTags,
    };

    fn app() -> AppState {
        AppState::new(
            &Config::default(),
            ScanIndex {
                generation: 1,
                complete: true,
                assets: Vec::new(),
                entries: Vec::new(),
                playlists: Vec::new(),
                warnings: Vec::new(),
                counters: ScanCounters::default(),
            },
        )
        .expect("app state")
    }

    fn root() -> (tempfile::TempDir, std::fs::File) {
        let root = tempfile::tempdir().expect("temporary root");
        fs::create_dir(root.path().join("state")).expect("state directory");
        fs::set_permissions(root.path().join("state"), fs::Permissions::from_mode(0o700))
            .expect("private state permissions");
        let root_file = std::fs::File::open(root.path()).expect("open root");
        (root, root_file)
    }

    fn app_with_control_metadata() -> AppState {
        let controls = "\u{1b}".repeat(4_096);
        let path = PathBuf::from("audio/library/fixture.wav");
        let index = ScanIndex {
            generation: 1,
            complete: true,
            assets: vec![MediaAsset {
                id: MediaAssetId(1),
                canonical_path: path.clone(),
                tags: TrackTags {
                    artist: Some(controls.clone()),
                    album_artist: None,
                    album: None,
                    title: Some(controls),
                },
                file_identity: FileIdentity {
                    device: 1,
                    inode: 1,
                    size: 1,
                    modified_seconds: 1,
                    modified_nanoseconds: 0,
                },
                external: false,
                cross_mount: false,
            }],
            entries: vec![TrackEntry {
                id: TrackEntryId(1),
                asset_id: MediaAssetId(1),
                display_path: PathBuf::from("library/fixture.wav"),
                source: TrackEntrySource::LibraryFile {
                    relative_path: PathBuf::from("library/fixture.wav"),
                },
                search: SearchFields {
                    metadata: Vec::new(),
                    filename: "fixture".into(),
                    relative_path: "library/fixture.wav".into(),
                },
                scan_generation: 1,
            }],
            playlists: Vec::new(),
            warnings: Vec::new(),
            counters: ScanCounters {
                encountered_entries: 1,
                ..ScanCounters::default()
            },
        };
        let mut app = AppState::new(&Config::default(), index).expect("app state");
        app.apply(AppAction::Activate)
            .expect("fixture track starts");
        app
    }

    #[test]
    fn projection_is_atomic_private_and_valid_json() {
        let (root, root_file) = root();
        let mut writer =
            NowPlayingWriter::open(root_file.as_fd(), root.path()).expect("state writer");

        let mut projection = NowPlayingProjection::from_app(&app());
        projection.title = "quiet\u{1b}[31m".into();
        writer.write(&projection).expect("write projection");

        let path = root.path().join("state/now-playing.json");
        let bytes = fs::read(&path).expect("read projection");
        assert!(!bytes.contains(&0x1b), "JSON must escape raw controls");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(value["version"], 1);
        assert_eq!(value["status"], "stopped");
        assert_eq!(value["speed_percent"], 100);
        assert_eq!(value["title"], "quiet\u{1b}[31m");
        assert_eq!(
            fs::metadata(path)
                .expect("state metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn unsafe_existing_projection_is_refused() {
        let (root, root_file) = root();
        symlink("/dev/null", root.path().join("state/now-playing.json")).expect("state symlink");

        let error = NowPlayingWriter::open(root_file.as_fd(), root.path())
            .err()
            .expect("symlink must be refused");

        assert!(error.to_string().contains("existing playback state"));
    }

    #[test]
    fn oversized_projection_does_not_replace_the_last_good_state() {
        let (root, root_file) = root();
        let mut writer =
            NowPlayingWriter::open(root_file.as_fd(), root.path()).expect("state writer");
        let valid = NowPlayingProjection::from_app(&app());
        writer.write(&valid).expect("initial projection");
        let path = root.path().join("state/now-playing.json");
        let before = fs::read(&path).expect("initial bytes");
        let mut oversized = valid;
        oversized.title = "x".repeat(20_000);

        assert!(writer.write(&oversized).is_err());
        assert_eq!(fs::read(path).expect("retained projection"), before);
    }

    #[test]
    fn maximally_escaped_identity_fields_fit_the_state_document() {
        let (root, root_file) = root();
        let mut writer =
            NowPlayingWriter::open(root_file.as_fd(), root.path()).expect("state writer");
        let projection = NowPlayingProjection::from_app(&app_with_control_metadata());

        writer
            .write(&projection)
            .expect("worst-case app projection remains encodable");

        let bytes =
            fs::read(root.path().join("state/now-playing.json")).expect("read bounded projection");
        assert!(bytes.len() <= STATE_MAX_BYTES, "{} bytes", bytes.len());
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(
            value["title"].as_str().map(str::len),
            Some(STATE_IDENTITY_MAX_BYTES)
        );
        assert_eq!(
            value["creator"].as_str().map(str::len),
            Some(STATE_IDENTITY_MAX_BYTES)
        );
    }
}
