// SPDX-License-Identifier: Apache-2.0

//! The private resume checkpoint, `state/session.json`.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};

use rustix::fd::AsFd;
use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::process::getuid;
use serde::{Deserialize, Serialize};

use crate::config::QueueConfig;
use crate::errors::{AppError, AppResult};

const STATE_VERSION: u32 = 1;
const SESSION_FILE: &str = "session.json";
const SESSION_MAX_BYTES: usize = 256 * 1_024;
const SESSION_MAX_POSITION_MS: u64 = 365 * 24 * 60 * 60 * 1_000;
/// Releases through 1.2 also wrote this playback summary. It is removed on open.
const RETIRED_NOW_PLAYING: &str = "now-playing.json";
// Recover from interrupted writes without letting unexpected directory contents
// turn temporary-name selection into unlimited work.
const STATE_TEMPORARY_CREATE_ATTEMPTS: usize = 16;

/// Minimal app-owned state needed to resume a listening session.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SessionSnapshot {
    version: u32,
    pub(crate) queue_entry_ids: Vec<u64>,
    pub(crate) current_index: usize,
    pub(crate) position_ms: u64,
}

impl SessionSnapshot {
    #[must_use]
    pub(crate) fn new(queue_entry_ids: Vec<u64>, current_index: usize, position_ms: u64) -> Self {
        Self {
            version: STATE_VERSION,
            queue_entry_ids,
            current_index,
            position_ms: position_ms.min(SESSION_MAX_POSITION_MS),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SessionLoad {
    Missing,
    Loaded(SessionSnapshot),
    Ignored(String),
}

/// Owns the verified private state directory for one terminal session.
pub(crate) struct StateStore {
    directory: File,
    directory_path: PathBuf,
    next_temporary: u64,
}

impl StateStore {
    pub(crate) fn open_file(&self, name: &str, description: &str) -> AppResult<Option<File>> {
        open_verified_state_file(&self.directory, &self.directory_path, name, description)
    }
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
        drop(open_verified_state_file(
            &directory,
            &directory_path,
            SESSION_FILE,
            "resume state",
        )?);
        Ok(Self {
            directory: File::from(directory),
            directory_path,
            next_temporary: 0,
        })
    }

    pub(crate) fn read_session(&self, queue: &QueueConfig) -> AppResult<SessionLoad> {
        let Some(file) = open_verified_state_file(
            &self.directory,
            &self.directory_path,
            SESSION_FILE,
            "resume state",
        )?
        else {
            return Ok(SessionLoad::Missing);
        };
        let mut bytes = Vec::new();
        file.take((SESSION_MAX_BYTES as u64).saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| {
                AppError::io(
                    "read resume state",
                    self.directory_path.join(SESSION_FILE),
                    error,
                )
            })?;
        if bytes.len() > SESSION_MAX_BYTES {
            return Ok(SessionLoad::Ignored(format!(
                "saved session exceeds the {SESSION_MAX_BYTES}-byte limit"
            )));
        }
        let snapshot = match serde_json::from_slice::<SessionSnapshot>(&bytes) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return Ok(SessionLoad::Ignored(format!(
                    "saved session is not valid JSON: {error}"
                )));
            }
        };
        if let Some(reason) = invalid_session_reason(&snapshot, queue) {
            return Ok(SessionLoad::Ignored(reason));
        }
        Ok(SessionLoad::Loaded(snapshot))
    }

    pub(crate) fn write_session(&mut self, snapshot: &SessionSnapshot) -> AppResult<()> {
        let mut bytes = serde_json::to_vec(snapshot)
            .map_err(|error| AppError::Resource(format!("cannot encode resume state: {error}")))?;
        bytes.push(b'\n');
        if bytes.len() > SESSION_MAX_BYTES {
            return Err(AppError::Resource(format!(
                "resume state exceeds the {SESSION_MAX_BYTES}-byte limit"
            )));
        }
        self.write_bytes(SESSION_FILE, "session", &bytes)
    }

    pub(crate) fn clear_session(&self) -> AppResult<()> {
        self.remove(SESSION_FILE, "remove resume state")
    }

    /// Removes state files that older releases wrote. Only the root's writer calls this.
    pub(crate) fn remove_retired(&self) -> AppResult<()> {
        self.remove(RETIRED_NOW_PLAYING, "remove retired playback state")
    }

    fn remove(&self, file_name: &str, operation: &'static str) -> AppResult<()> {
        match rustix::fs::unlinkat(&self.directory, file_name, AtFlags::empty()) {
            Ok(()) => Ok(()),
            Err(error) if error == rustix::io::Errno::NOENT => Ok(()),
            Err(error) => Err(AppError::io(
                operation,
                self.directory_path.join(file_name),
                error.into(),
            )),
        }
    }

    pub(crate) fn write_bytes(
        &mut self,
        file_name: &str,
        temporary_stem: &str,
        bytes: &[u8],
    ) -> AppResult<()> {
        let (temporary, temporary_path, mut file) = self.create_temporary(temporary_stem)?;
        let result = (|| {
            rustix::fs::fchmod(&file, Mode::from_raw_mode(0o600)).map_err(|error| {
                AppError::io("set state permissions", &temporary_path, error.into())
            })?;
            // Readers need one complete snapshot; crash durability is unnecessary for ephemeral state.
            file.write_all(bytes)
                .map_err(|error| AppError::io("write state", &temporary_path, error))?;
            rustix::fs::renameat(&self.directory, &temporary, &self.directory, file_name).map_err(
                |error| {
                    AppError::io(
                        "replace state",
                        self.directory_path.join(file_name),
                        error.into(),
                    )
                },
            )?;
            Ok(())
        })();
        if result.is_err() {
            let _ = rustix::fs::unlinkat(&self.directory, &temporary, AtFlags::empty());
        }
        result
    }

    fn create_temporary(&mut self, stem: &str) -> AppResult<(String, PathBuf, File)> {
        for _ in 0..STATE_TEMPORARY_CREATE_ATTEMPTS {
            let temporary = format!(".{stem}.{}.{}.tmp", std::process::id(), self.next_temporary);
            self.next_temporary += 1;
            let temporary_path = self.directory_path.join(&temporary);
            match rustix::fs::openat(
                &self.directory,
                &temporary,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o600),
            ) {
                Ok(fd) => return Ok((temporary, temporary_path, File::from(fd))),
                Err(error) if error == rustix::io::Errno::EXIST => {}
                Err(error) => {
                    return Err(AppError::io(
                        "create temporary state",
                        temporary_path,
                        error.into(),
                    ));
                }
            }
        }
        Err(AppError::Resource(format!(
            "cannot create temporary state after {STATE_TEMPORARY_CREATE_ATTEMPTS} name collisions"
        )))
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

fn open_verified_state_file(
    directory: impl AsFd,
    directory_path: &Path,
    file_name: &str,
    description: &str,
) -> AppResult<Option<File>> {
    let fd = match rustix::fs::openat(
        directory,
        file_name,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(error) if error == rustix::io::Errno::NOENT => return Ok(None),
        Err(error) => {
            return Err(AppError::io(
                "open existing state",
                directory_path.join(file_name),
                error.into(),
            ));
        }
    };
    let stat = rustix::fs::fstat(&fd).map_err(|error| {
        AppError::io(
            "inspect existing state",
            directory_path.join(file_name),
            error.into(),
        )
    })?;
    if !FileType::from_raw_mode(stat.st_mode).is_file()
        || stat.st_uid != getuid().as_raw()
        || stat.st_mode & 0o777 != 0o600
        || stat.st_nlink != 1
    {
        return Err(AppError::InvalidRoot {
            path: directory_path.join(file_name),
            reason: format!(
                "{description} must be a current-user-owned regular file with mode 0600 and one link"
            ),
        });
    }
    Ok(Some(File::from(fd)))
}

fn invalid_session_reason(snapshot: &SessionSnapshot, queue: &QueueConfig) -> Option<String> {
    if snapshot.version != STATE_VERSION {
        Some(format!(
            "saved session version {} is unsupported",
            snapshot.version
        ))
    } else if snapshot.queue_entry_ids.is_empty() {
        Some("saved session queue is empty".into())
    } else if snapshot.queue_entry_ids.len() > queue.max_items {
        Some("saved session queue exceeds the queue limit".into())
    } else if snapshot.current_index >= snapshot.queue_entry_ids.len() {
        Some("saved session current track is outside the queue".into())
    } else if snapshot.position_ms > SESSION_MAX_POSITION_MS {
        Some("saved session position exceeds one year".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use rustix::fd::AsFd;

    use super::{STATE_TEMPORARY_CREATE_ATTEMPTS, SessionLoad, SessionSnapshot, StateStore};
    use crate::config::Config;

    fn root() -> (tempfile::TempDir, std::fs::File) {
        let root = tempfile::tempdir().expect("temporary root");
        fs::create_dir(root.path().join("state")).expect("state directory");
        fs::set_permissions(root.path().join("state"), fs::Permissions::from_mode(0o700))
            .expect("private state permissions");
        let root_file = std::fs::File::open(root.path()).expect("open root");
        (root, root_file)
    }

    #[test]
    fn abandoned_temporary_state_does_not_block_atomic_replacement() {
        let (root, root_file) = root();
        let abandoned = root
            .path()
            .join(format!("state/.session.{}.0.tmp", std::process::id()));
        fs::write(&abandoned, b"abandoned").expect("abandoned temporary state");
        fs::set_permissions(&abandoned, fs::Permissions::from_mode(0o600))
            .expect("private temporary state permissions");
        let mut store = StateStore::open(root_file.as_fd(), root.path()).expect("state store");

        store
            .write_session(&SessionSnapshot::new(vec![1], 0, 0))
            .expect("skip abandoned temporary state");

        assert_eq!(
            fs::read(&abandoned).expect("abandoned state remains untouched"),
            b"abandoned"
        );
        assert!(matches!(
            store
                .read_session(&Config::default().queue)
                .expect("read replacement"),
            SessionLoad::Loaded(_)
        ));
    }

    #[test]
    fn temporary_state_name_collisions_stop_at_the_retry_bound() {
        let (root, root_file) = root();
        for counter in 0..STATE_TEMPORARY_CREATE_ATTEMPTS {
            let collision = root.path().join(format!(
                "state/.session.{}.{counter}.tmp",
                std::process::id()
            ));
            fs::write(collision, b"occupied").expect("occupied temporary name");
        }
        let mut store = StateStore::open(root_file.as_fd(), root.path()).expect("state store");

        let error = store
            .write_session(&SessionSnapshot::new(vec![1], 0, 0))
            .expect_err("bounded collisions must fail");

        assert!(error.to_string().contains("after 16 name collisions"));
        assert!(!root.path().join("state/session.json").exists());
    }

    #[test]
    fn retired_playback_state_is_removed_only_on_request() {
        let (root, root_file) = root();
        let retired = root.path().join("state/now-playing.json");
        fs::write(&retired, b"{}").expect("retired state fixture");

        let store = StateStore::open(root_file.as_fd(), root.path()).expect("state store");
        assert!(retired.exists(), "opening the store changes nothing");
        store.remove_retired().expect("remove retired state");

        assert!(!retired.exists());
    }

    #[test]
    fn resume_state_round_trips_privately_and_clears() {
        let (root, root_file) = root();
        let mut store = StateStore::open(root_file.as_fd(), root.path()).expect("state store");
        let snapshot = SessionSnapshot::new(vec![11, 22, 11], 1, 93_250);

        store.write_session(&snapshot).expect("write resume state");

        assert_eq!(
            store
                .read_session(&Config::default().queue)
                .expect("read resume state"),
            SessionLoad::Loaded(snapshot)
        );
        let path = root.path().join("state/session.json");
        assert_eq!(
            fs::metadata(&path)
                .expect("resume metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        store.clear_session().expect("clear resume state");
        assert_eq!(
            store
                .read_session(&Config::default().queue)
                .expect("missing resume state"),
            SessionLoad::Missing
        );
    }

    #[test]
    fn maximum_configured_queue_fits_the_resume_document_budget() {
        let (root, root_file) = root();
        let config = Config::default();
        let mut store = StateStore::open(root_file.as_fd(), root.path()).expect("state store");
        let snapshot = SessionSnapshot::new(
            vec![u64::MAX; config.queue.max_items],
            config.queue.max_items - 1,
            1,
        );

        store
            .write_session(&snapshot)
            .expect("maximum queue checkpoint");

        let bytes = fs::read(root.path().join("state/session.json")).expect("resume bytes");
        assert!(
            bytes.len() <= super::SESSION_MAX_BYTES,
            "{} bytes",
            bytes.len()
        );
        assert!(matches!(
            store.read_session(&config.queue).expect("read maximum queue"),
            SessionLoad::Loaded(loaded) if loaded == snapshot
        ));
    }

    #[test]
    fn malformed_resume_state_is_bounded_and_ignored() {
        let (root, root_file) = root();
        let path = root.path().join("state/session.json");
        fs::write(&path, br#"{"version":1,"queue_entry_ids":[1]}"#)
            .expect("malformed resume fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .expect("private resume permissions");
        let store = StateStore::open(root_file.as_fd(), root.path()).expect("state store");

        assert!(matches!(
            store
                .read_session(&Config::default().queue)
                .expect("read malformed resume state"),
            SessionLoad::Ignored(reason) if reason.contains("not valid JSON")
        ));
    }

    #[test]
    fn unsafe_existing_resume_state_is_refused() {
        let (root, root_file) = root();
        symlink("/dev/null", root.path().join("state/session.json")).expect("state symlink");

        let error = StateStore::open(root_file.as_fd(), root.path())
            .err()
            .expect("resume symlink must be refused");

        assert!(error.to_string().contains("session.json"));
    }
}
