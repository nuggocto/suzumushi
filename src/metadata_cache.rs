// SPDX-License-Identifier: Apache-2.0

//! Disposable metadata cache. Media descriptors remain the source of identity.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read};
use std::os::fd::BorrowedFd;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::metadata::{HelperMetadataReader, MetadataReader};
use crate::model::TrackTags;
use crate::state::StateStore;

const CACHE_FILE: &str = "metadata.jsonl";
// A release may change parser behavior even when the on-disk record shape is unchanged.
const HEADER: &str = concat!("suzumushi-metadata-v1-", env!("CARGO_PKG_VERSION"), "\n");
const MAX_CACHE_BYTES: usize = 4 * 1_048_576;
const MAX_ENTRIES: usize = 10_000;
const MAX_RECORD_BYTES: usize = 128 * 1_024;
const MAX_FIELD_BYTES: usize = 16_384;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl Identity {
    fn read(file: &File) -> io::Result<Self> {
        let stat = file.metadata()?;
        Ok(Self {
            device: stat.dev(),
            inode: stat.ino(),
            size: stat.size(),
            modified_seconds: stat.mtime(),
            modified_nanoseconds: stat.mtime_nsec(),
            changed_seconds: stat.ctime(),
            changed_nanoseconds: stat.ctime_nsec(),
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Record {
    identity: Identity,
    tags: TrackTags,
}

#[derive(Serialize)]
struct BorrowedRecord<'a> {
    identity: &'a Identity,
    tags: &'a TrackTags,
}

struct CachedTags {
    tags: TrackTags,
    used: bool,
}

#[derive(Default)]
pub(crate) struct CachedMetadataReader {
    writable: bool,
    entries: BTreeMap<Identity, CachedTags>,
    tag_bytes: usize,
}

impl CachedMetadataReader {
    pub(crate) fn load(root: BorrowedFd<'_>, path: &Path) -> Self {
        // Cache failures must never prevent playback or make diagnose mutate state.
        let Ok(store) = StateStore::open(root, path) else {
            return Self::default();
        };
        let mut cache = match store.open_file(CACHE_FILE, "metadata cache") {
            Ok(Some(file)) => match Self::read_records(file) {
                Ok(cache) => cache,
                Err(error) => {
                    tracing::debug!(%error, "ignoring invalid metadata cache");
                    Self::default()
                }
            },
            Ok(None) => Self::default(),
            // Do not overwrite a symlink, hard link, special file or exposed cache.
            Err(_) => return Self::default(),
        };
        cache.writable = true;
        cache
    }

    fn read_records(reader: impl Read) -> io::Result<Self> {
        let mut reader = BufReader::new(reader.take((MAX_CACHE_BYTES + 1) as u64));
        let mut line = Vec::new();
        let mut total = read_line(&mut reader, &mut line)?;
        if line != HEADER.as_bytes() {
            return Err(io::Error::other("metadata cache version mismatch"));
        }
        let mut cache = Self::default();
        loop {
            let read = read_line(&mut reader, &mut line)?;
            if read == 0 {
                return Ok(cache);
            }
            total += read;
            if total > MAX_CACHE_BYTES || cache.entries.len() == MAX_ENTRIES {
                return Err(io::Error::other("metadata cache limit exceeded"));
            }
            let record: Record = serde_json::from_slice(&line).map_err(io::Error::other)?;
            if cache.entries.contains_key(&record.identity)
                || !cache.insert(record.identity, record.tags, false)
            {
                return Err(io::Error::other("invalid metadata cache record"));
            }
        }
    }

    fn insert(&mut self, identity: Identity, tags: TrackTags, used: bool) -> bool {
        let fields = [&tags.artist, &tags.album_artist, &tags.album, &tags.title];
        if fields
            .into_iter()
            .flatten()
            .any(|text| text.len() > MAX_FIELD_BYTES)
        {
            return false;
        }
        let bytes = fields
            .into_iter()
            .flatten()
            .map(String::capacity)
            .sum::<usize>();
        if self.entries.len() >= MAX_ENTRIES || bytes > MAX_CACHE_BYTES - self.tag_bytes {
            return false;
        }
        self.tag_bytes += bytes;
        self.entries.insert(identity, CachedTags { tags, used });
        true
    }

    fn read_with(
        &mut self,
        file: &File,
        reader: &mut impl MetadataReader,
    ) -> Result<TrackTags, String> {
        let identity = Identity::read(file).map_err(|error| error.to_string())?;
        if let Some(cached) = self.entries.get_mut(&identity) {
            cached.used = true;
            return Ok(cached.tags.clone());
        }
        let tags = reader.read(file)?;
        // A file edited during parsing must not seed a reusable cache entry.
        if Identity::read(file).is_ok_and(|after| after == identity) {
            self.insert(identity, tags.clone(), true);
        }
        Ok(tags)
    }

    /// Called only by the terminal startup while holding the root writer lease.
    pub(crate) fn save(&mut self, root: BorrowedFd<'_>, path: &Path) {
        if let Err(error) = self.save_inner(root, path) {
            tracing::warn!(%error, "metadata cache update failed; playback continues");
        }
    }

    fn save_inner(&mut self, root: BorrowedFd<'_>, path: &Path) -> crate::errors::AppResult<()> {
        if !self.writable {
            return Ok(());
        }
        let mut store = StateStore::open(root, path)?;
        drop(store.open_file(CACHE_FILE, "metadata cache")?);
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(MAX_CACHE_BYTES).map_err(|error| {
            crate::errors::AppError::Resource(format!("cannot reserve metadata cache: {error}"))
        })?;
        bytes.extend_from_slice(HEADER.as_bytes());
        for (identity, cached) in &self.entries {
            if !cached.used {
                continue;
            }
            // Borrow the tags so serialization does not clone the retained cache.
            let line = serde_json::to_vec(&BorrowedRecord {
                identity,
                tags: &cached.tags,
            })
            .map_err(|error| crate::errors::AppError::Resource(error.to_string()))?;
            if line.len() + 1 > MAX_RECORD_BYTES || bytes.len() + line.len() + 1 > MAX_CACHE_BYTES {
                continue;
            }
            bytes.extend_from_slice(&line);
            bytes.push(b'\n');
        }
        store.write_bytes(CACHE_FILE, "metadata", &bytes)
    }
}

impl MetadataReader for CachedMetadataReader {
    fn cached(&mut self, file: &File) -> Option<TrackTags> {
        let identity = Identity::read(file).ok()?;
        let cached = self.entries.get_mut(&identity)?;
        cached.used = true;
        Some(cached.tags.clone())
    }

    fn read(&mut self, file: &File) -> Result<TrackTags, String> {
        self.read_with(file, &mut HelperMetadataReader)
    }
}

fn read_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<usize> {
    line.clear();
    let read = reader
        .take((MAX_RECORD_BYTES + 1) as u64)
        .read_until(b'\n', line)?;
    if read > MAX_RECORD_BYTES || (read != 0 && line.last() != Some(&b'\n')) {
        return Err(io::Error::other("invalid metadata cache line"));
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::fd::AsFd;
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    struct TagsReader {
        attempts: usize,
        title: &'static str,
    }

    impl MetadataReader for TagsReader {
        fn read(&mut self, _file: &File) -> Result<TrackTags, String> {
            self.attempts += 1;
            Ok(TrackTags {
                title: Some(self.title.into()),
                ..TrackTags::default()
            })
        }
    }

    fn root() -> (tempfile::TempDir, File, File) {
        let temp = tempfile::tempdir().expect("temporary root");
        crate::init::initialize(temp.path()).expect("initialize root");
        let media = temp.path().join("audio/library/tone.mp3");
        fs::write(&media, b"fixture").expect("media fixture");
        let root = File::open(temp.path()).expect("root descriptor");
        let file = File::open(media).expect("media descriptor");
        (temp, root, file)
    }

    #[test]
    fn unchanged_descriptors_reuse_private_cache_and_changed_files_are_reparsed() {
        let (temp, root, file) = root();
        let mut parser = TagsReader {
            attempts: 0,
            title: "first",
        };
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        let tags = cache.read_with(&file, &mut parser).expect("initial parse");
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("cache checkpoint");
        let path = temp.path().join("state/metadata.jsonl");
        assert_eq!(
            fs::metadata(&path)
                .expect("cache metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        assert_eq!(
            cache.read_with(&file, &mut parser).expect("cache hit"),
            tags
        );
        assert_eq!(parser.attempts, 1);
        // Rewriting the file to a different size deterministically changes its identity.
        fs::write(
            temp.path().join("audio/library/tone.mp3"),
            b"changed fixture",
        )
        .expect("edit media");
        parser.title = "second";
        assert_eq!(
            cache
                .read_with(&file, &mut parser)
                .expect("cache miss")
                .title
                .as_deref(),
            Some("second")
        );
        assert_eq!(parser.attempts, 2);

        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("discard entries not seen in the new scan");
        assert_eq!(fs::read(path).expect("pruned cache"), HEADER.as_bytes());
    }

    #[test]
    fn cache_hits_obey_current_metadata_limits_without_starting_helpers() {
        struct Observer;
        impl crate::scan::ScanObserver for Observer {}
        let (temp, root, file) = root();
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache
            .read_with(
                &file,
                &mut TagsReader {
                    attempts: 0,
                    title: "cached title",
                },
            )
            .expect("seed cache");
        let mut config = crate::config::Config::default();
        config.scan.max_parser_attempts = 0;
        let scan = |config: &crate::config::Config, cache: &mut CachedMetadataReader| {
            crate::scan::scan_with_components(
                temp.path(),
                config,
                1,
                crate::scan::ScanOptions::default(),
                cache,
                &mut Observer,
            )
            .expect("scan cached fixture")
        };
        let index = scan(&config, &mut cache);
        assert!(index.complete);
        assert_eq!(index.counters.parser_attempts, 0);
        assert_eq!(index.assets[0].tags.title.as_deref(), Some("cached title"));
        config.scan.max_metadata_field_bytes = 4;
        let index = scan(&config, &mut cache);
        assert!(index.assets[0].tags.title.is_none());
        assert!(
            index
                .warnings
                .iter()
                .any(|warning| warning.code == crate::model::ScanWarningCode::MetadataRead)
        );
    }

    #[test]
    fn parser_side_edits_are_not_cached() {
        struct EditingReader;
        impl MetadataReader for EditingReader {
            fn read(&mut self, file: &File) -> Result<TrackTags, String> {
                file.set_len(10).expect("edit during parse");
                Ok(TrackTags::default())
            }
        }
        let file = tempfile::tempfile().expect("writable descriptor");
        let mut cache = CachedMetadataReader::default();
        cache
            .read_with(&file, &mut EditingReader)
            .expect("parser result remains usable");
        assert_eq!(
            cache.cached(&file),
            None,
            "racing parse must not become a cache hit"
        );
    }

    #[test]
    fn malformed_cache_is_rebuilt_but_unsafe_cache_targets_are_untouched() {
        let (temp, root, file) = root();
        let path = temp.path().join("state/metadata.jsonl");
        fs::write(&path, b"invalid cache").expect("malformed fixture");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("private cache");
        let mut parser = TagsReader {
            attempts: 0,
            title: "recovered",
        };
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache
            .read_with(&file, &mut parser)
            .expect("fall back to parser");
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("rebuild cache");
        assert!(
            fs::read(&path)
                .expect("cache")
                .starts_with(HEADER.as_bytes())
        );

        fs::remove_file(&path).expect("replace with symlink");
        let sentinel = temp.path().join("sentinel");
        fs::write(&sentinel, b"unchanged").expect("sentinel");
        symlink(&sentinel, &path).expect("cache symlink");
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache
            .read_with(&file, &mut parser)
            .expect("unsafe cache is a miss");
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("disabled cache write");
        assert!(
            fs::symlink_metadata(path)
                .expect("symlink remains")
                .is_symlink()
        );
        assert_eq!(fs::read(sentinel).expect("sentinel remains"), b"unchanged");
    }

    #[test]
    fn cache_reader_enforces_total_bytes_and_entry_count() {
        let (_temp, _root, file) = root();
        let mut record = Record {
            identity: Identity::read(&file).expect("identity"),
            tags: TrackTags::default(),
        };
        let mut bytes = HEADER.as_bytes().to_vec();
        for inode in 0..MAX_ENTRIES {
            record.identity.inode = inode as u64;
            serde_json::to_writer(&mut bytes, &record).expect("record JSON");
            bytes.push(b'\n');
        }
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_ok(),
            "exact entry count"
        );
        record.identity.inode = MAX_ENTRIES as u64;
        serde_json::to_writer(&mut bytes, &record).expect("extra record");
        bytes.push(b'\n');
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_err(),
            "entry count plus one"
        );

        bytes.clear();
        bytes.extend_from_slice(HEADER.as_bytes());
        for inode in 0..(MAX_CACHE_BYTES / MAX_RECORD_BYTES) {
            record.identity.inode = inode as u64;
            let line_size = MAX_RECORD_BYTES.min(MAX_CACHE_BYTES - bytes.len());
            let end = bytes.len() + line_size;
            serde_json::to_writer(&mut bytes, &record).expect("padded record");
            bytes.resize(end - 1, b' ');
            bytes.push(b'\n');
        }
        assert_eq!(bytes.len(), MAX_CACHE_BYTES);
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_ok(),
            "exact file size"
        );
        bytes.push(b'\n');
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_err(),
            "file size plus one"
        );
    }

    #[test]
    fn cache_reader_bounds_records_and_rejects_duplicate_identities() {
        let (_temp, _root, file) = root();
        let record = Record {
            identity: Identity::read(&file).expect("identity"),
            tags: TrackTags::default(),
        };
        let mut line = serde_json::to_vec(&record).expect("record JSON");
        line.resize(MAX_RECORD_BYTES - 1, b' ');
        line.push(b'\n');
        let mut bytes = HEADER.as_bytes().to_vec();
        bytes.extend_from_slice(&line);
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_ok(),
            "exact record bound"
        );
        bytes.push(b' ');
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_err(),
            "truncated final record"
        );
        bytes.pop();
        bytes.extend_from_slice(&line);
        assert!(
            CachedMetadataReader::read_records(bytes.as_slice()).is_err(),
            "duplicate identity"
        );

        let mut oversized = HEADER.as_bytes().to_vec();
        oversized.extend(std::iter::repeat_n(b' ', MAX_RECORD_BYTES));
        oversized.push(b'\n');
        assert!(
            CachedMetadataReader::read_records(oversized.as_slice()).is_err(),
            "record limit plus one"
        );
        assert!(CachedMetadataReader::read_records(b"obsolete version\n".as_slice()).is_err());
    }
}
