// SPDX-License-Identifier: Apache-2.0

//! Descriptor-relative playback opens with scan-identity revalidation.

use std::ffi::OsStr;
use std::fs::File;
use std::os::fd::AsFd;
use std::path::{Component, Path};

use rustix::fd::{BorrowedFd, OwnedFd};
use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};

use crate::errors::{AppError, AppResult};
use crate::model::{MediaAsset, TrackEntry, TrackEntrySource};

/// Opens one scanned entry from the pinned root and proves that it is still the same file.
///
/// # Errors
///
/// Returns an audio error when the contextual path changed, traverses a directory
/// symlink, names a non-regular file, or no longer matches the scanned identity.
pub(crate) fn open_verified_media(
    root: BorrowedFd<'_>,
    entry: &TrackEntry,
    asset: &MediaAsset,
) -> AppResult<File> {
    let mut components = entry.display_path.components().peekable();
    let mut directory = open_directory(root, OsStr::new("audio"))
        .map_err(|error| AppError::Audio(format!("cannot open the audio root: {error}")))?;
    let mut final_name = None;
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err(AppError::Audio(
                "track path contains a non-normal component; rescan required".into(),
            ));
        };
        if components.peek().is_none() {
            final_name = Some(name);
            break;
        }
        directory = open_directory(&directory, name).map_err(|error| {
            AppError::Audio(format!(
                "track directory changed before playback; rescan required: {error}"
            ))
        })?;
    }
    let final_name = final_name.ok_or_else(|| AppError::Audio("track path is empty".into()))?;
    let follows_file_link = matches!(
        entry.source,
        TrackEntrySource::LibrarySymlink { .. } | TrackEntrySource::PlaylistSymlink { .. }
    );
    let fd = if follows_file_link {
        rustix::fs::openat2(
            &directory,
            final_name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            ResolveFlags::NO_MAGICLINKS,
        )
        .map_err(|error| {
            AppError::Audio(format!(
                "cannot securely reopen the file symlink; rescan required: {error}"
            ))
        })?
    } else {
        rustix::fs::openat(
            &directory,
            final_name,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|error| {
            AppError::Audio(format!("cannot reopen the track; rescan required: {error}"))
        })?
    };
    let stat = rustix::fs::fstat(&fd)
        .map_err(|error| AppError::Audio(format!("cannot inspect the reopened track: {error}")))?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err(AppError::Audio(
            "reopened track is not a regular file; rescan required".into(),
        ));
    }
    let size = u64::try_from(stat.st_size)
        .map_err(|_| AppError::Audio("reopened track reported a negative size".into()))?;
    let identity = &asset.file_identity;
    if stat.st_dev != identity.device
        || stat.st_ino != identity.inode
        || size != identity.size
        || stat.st_mtime != identity.modified_seconds
        || stat.st_mtime_nsec != identity.modified_nanoseconds
    {
        return Err(AppError::Audio(
            "track changed after the library scan; rescan required".into(),
        ));
    }
    Ok(File::from(fd))
}

fn open_directory<Fd: AsFd>(parent: Fd, name: &OsStr) -> rustix::io::Result<OwnedFd> {
    rustix::fs::openat(
        parent,
        Path::new(name),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, symlink};
    use std::path::{Path, PathBuf};

    use tempfile::TempDir;

    use super::open_verified_media;
    use crate::model::{
        FileIdentity, MediaAsset, MediaAssetId, SearchFields, TrackEntry, TrackEntryId,
        TrackEntrySource, TrackTags,
    };
    use crate::paths::discover_root_from;

    fn fixture(root: &Path, relative: &str) -> (TrackEntry, MediaAsset) {
        let path = root.join("audio").join(relative);
        let metadata = fs::metadata(&path).expect("fixture media metadata");
        (
            TrackEntry {
                id: TrackEntryId(1),
                asset_id: MediaAssetId(2),
                display_path: PathBuf::from(relative),
                source: TrackEntrySource::LibraryFile {
                    relative_path: PathBuf::from(relative),
                },
                search: SearchFields {
                    metadata: Vec::new(),
                    filename: "tone".into(),
                    relative_path: relative.into(),
                },
                scan_generation: 1,
            },
            MediaAsset {
                id: MediaAssetId(2),
                canonical_path: path,
                tags: TrackTags::default(),
                file_identity: FileIdentity {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    size: metadata.size(),
                    modified_seconds: metadata.mtime(),
                    modified_nanoseconds: u64::try_from(metadata.mtime_nsec())
                        .expect("fixture nanoseconds are non-negative"),
                },
                external: false,
                cross_mount: false,
            },
        )
    }

    #[test]
    fn playback_open_revalidates_the_scanned_file_identity() {
        let temp = TempDir::new().expect("temporary directory");
        let root = temp.path().join("root");
        fs::create_dir_all(root.join("audio/library")).expect("create audio hierarchy");
        let path = root.join("audio/library/tone.wav");
        fs::write(&path, b"first identity").expect("write original media");
        let (entry, asset) = fixture(&root, "library/tone.wav");
        let selected = discover_root_from(Some(&root), None, temp.path()).expect("select root");

        open_verified_media(selected.descriptor(), &entry, &asset)
            .expect("unchanged track reopens");
        fs::remove_file(&path).expect("remove original media");
        fs::write(&path, b"replacement identity").expect("write replacement media");

        let error = open_verified_media(selected.descriptor(), &entry, &asset)
            .expect_err("replacement must not inherit scan admission");
        assert!(error.to_string().contains("changed after the library scan"));
    }

    #[test]
    fn playback_open_never_traverses_a_substituted_directory_symlink() {
        let temp = TempDir::new().expect("temporary directory");
        let root = temp.path().join("root");
        let genre = root.join("audio/library/genre");
        fs::create_dir_all(&genre).expect("create audio hierarchy");
        fs::write(genre.join("tone.wav"), b"original").expect("write original media");
        let (entry, asset) = fixture(&root, "library/genre/tone.wav");
        let selected = discover_root_from(Some(&root), None, temp.path()).expect("select root");
        let outside = temp.path().join("outside");
        fs::create_dir(&outside).expect("create outside directory");
        fs::write(outside.join("tone.wav"), b"replacement").expect("write outside media");
        fs::rename(&genre, root.join("audio/library/held")).expect("move admitted directory");
        symlink(&outside, &genre).expect("substitute directory symlink");

        let error = open_verified_media(selected.descriptor(), &entry, &asset)
            .expect_err("directory symlink must not be traversed");
        assert!(error.to_string().contains("track directory changed"));
    }
}
