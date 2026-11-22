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
/// Matches the scanner's file limit. The tag text and encoded file have
/// separate byte limits.
const MAX_ENTRIES: usize = 50_000;
/// Retained tag text across all records.
const MAX_TAG_BYTES: usize = 16 * 1_048_576;
/// The whole file: the tag budget plus each record's identity and JSON framing,
/// at most 337 bytes with every number at its widest and every tag absent.
/// JSON escaping can still lengthen tag text, so `save` skips records that
/// would exceed this limit.
const MAX_CACHE_BYTES: usize = MAX_TAG_BYTES + MAX_ENTRIES * 384;
const MAX_RECORD_BYTES: usize = 128 * 1_024;
const MAX_FIELD_BYTES: usize = 16_384;
const _: () = assert!(MAX_RECORD_BYTES <= MAX_CACHE_BYTES);
const _: () = assert!(4 * MAX_FIELD_BYTES < MAX_RECORD_BYTES);

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
    /// Seen in this scan and admitted to the next cache file.
    retained: bool,
}

/// Entry and tag-text limits for one cache file's worth of records.
#[derive(Default)]
struct Budget {
    entries: usize,
    tag_bytes: usize,
}

impl Budget {
    /// Charges one record of `tag_bytes`, or refuses it when either limit
    /// would be passed.
    fn admit(&mut self, tag_bytes: usize) -> bool {
        if self.entries >= MAX_ENTRIES || tag_bytes > MAX_TAG_BYTES - self.tag_bytes {
            return false;
        }
        self.entries += 1;
        self.tag_bytes += tag_bytes;
        true
    }
}

/// Loaded records are charged to `loaded`. Records this scan uses, whether
/// loaded or freshly parsed, are charged to `retained`, the budget of the file
/// `save` writes. Stale loaded records therefore never crowd out fresh ones,
/// and memory stays within two files' worth of records.
#[derive(Default)]
pub(crate) struct CachedMetadataReader {
    writable: bool,
    entries: BTreeMap<Identity, CachedTags>,
    loaded: Budget,
    retained: Budget,
    /// Whether parsing or rebuilding added records that may change the file.
    changed: bool,
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
                    crate::logging::info(format_args!("ignoring invalid metadata cache: {error}"));
                    Self::rebuilt()
                }
            },
            Ok(None) => Self::rebuilt(),
            // Do not overwrite a symlink, hard link, special file or exposed cache.
            Err(_) => return Self::default(),
        };
        cache.writable = true;
        cache
    }

    /// A cache with no usable file behind it, which `save` must write.
    fn rebuilt() -> Self {
        Self {
            changed: true,
            ..Self::default()
        }
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
            let admitted = tag_bytes(&record.tags).is_some_and(|bytes| cache.loaded.admit(bytes));
            if !admitted || cache.entries.contains_key(&record.identity) {
                return Err(io::Error::other("invalid metadata cache record"));
            }
            let cached = CachedTags {
                tags: record.tags,
                retained: false,
            };
            cache.entries.insert(record.identity, cached);
        }
    }

    /// Returns the cached tags for `identity`, retaining them for the next
    /// cache file when its budget allows.
    fn hit(&mut self, identity: &Identity) -> Option<TrackTags> {
        let cached = self.entries.get_mut(identity)?;
        if !cached.retained {
            let bytes = tag_bytes(&cached.tags).unwrap_or(usize::MAX);
            cached.retained = self.retained.admit(bytes);
        }
        Some(cached.tags.clone())
    }

    /// Remembers freshly parsed tags when the next cache file has room.
    fn insert_parsed(&mut self, identity: Identity, tags: TrackTags) {
        let admitted = tag_bytes(&tags).is_some_and(|bytes| self.retained.admit(bytes));
        if admitted {
            let cached = CachedTags {
                tags,
                retained: true,
            };
            self.entries.insert(identity, cached);
            self.changed = true;
        }
    }

    fn read_with(
        &mut self,
        file: &File,
        reader: &mut impl MetadataReader,
    ) -> Result<TrackTags, String> {
        let identity = Identity::read(file).map_err(|error| error.to_string())?;
        if let Some(tags) = self.hit(&identity) {
            return Ok(tags);
        }
        let tags = reader.read(file)?;
        // A file edited during parsing must not seed a reusable cache entry.
        if Identity::read(file).is_ok_and(|after| after == identity) {
            self.insert_parsed(identity, tags.clone());
        }
        Ok(tags)
    }

    /// Called only by the terminal startup while holding the root writer lease.
    pub(crate) fn save(&mut self, root: BorrowedFd<'_>, path: &Path) {
        if let Err(error) = self.save_inner(root, path) {
            crate::logging::warn(format_args!(
                "metadata cache update failed; playback continues: {error}"
            ));
        }
    }

    fn save_inner(&mut self, root: BorrowedFd<'_>, path: &Path) -> crate::errors::AppResult<()> {
        // Records this scan did not retain would be dropped from the file.
        let dropping = self.entries.values().any(|cached| !cached.retained);
        if !self.writable || !(self.changed || dropping) {
            return Ok(());
        }
        let mut store = StateStore::open(root, path)?;
        let existing = store.open_file(CACHE_FILE, "metadata cache")?;
        let mut bytes = Vec::new();
        let estimated = HEADER
            .len()
            .saturating_add(self.entries.len().saturating_mul(256))
            .min(MAX_CACHE_BYTES);
        bytes.try_reserve_exact(estimated).map_err(|error| {
            crate::errors::AppError::Resource(format!("cannot reserve metadata cache: {error}"))
        })?;
        bytes.extend_from_slice(HEADER.as_bytes());
        for (identity, cached) in &self.entries {
            if !cached.retained {
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
        // Some newly parsed records can be too large once JSON-encoded. Check
        // the actual output before replacing the file.
        if let Some(file) = existing
            && cache_matches(BufReader::new(file), &bytes).map_err(|error| {
                crate::errors::AppError::io(
                    "compare metadata cache",
                    path.join("state").join(CACHE_FILE),
                    error,
                )
            })?
        {
            return Ok(());
        }
        store.write_bytes(CACHE_FILE, "metadata", &bytes)
    }
}

/// Compares at most one buffer beyond the size of the candidate.
fn cache_matches(mut existing: impl BufRead, mut candidate: &[u8]) -> io::Result<bool> {
    loop {
        let chunk = existing.fill_buf()?;
        if chunk.is_empty() {
            return Ok(candidate.is_empty());
        }
        let Some(remaining) = candidate.strip_prefix(chunk) else {
            return Ok(false);
        };
        candidate = remaining;
        let matched = chunk.len();
        existing.consume(matched);
    }
}

impl MetadataReader for CachedMetadataReader {
    fn cached(&mut self, file: &File) -> Option<TrackTags> {
        let identity = Identity::read(file).ok()?;
        self.hit(&identity)
    }

    fn read(&mut self, file: &File) -> Result<TrackTags, String> {
        self.read_with(file, &mut HelperMetadataReader)
    }
}

/// A record's tag text in bytes, or `None` when a field is longer than any
/// record may hold. Lengths rather than capacities, so a record charges the
/// same before saving and after loading.
fn tag_bytes(tags: &TrackTags) -> Option<usize> {
    let fields = [&tags.artist, &tags.album_artist, &tags.album, &tags.title];
    let mut total = 0_usize;
    for text in fields.into_iter().flatten() {
        if text.len() > MAX_FIELD_BYTES {
            return None;
        }
        total += text.len();
    }
    Some(total)
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

    fn seed_stale_cache(path: &std::path::Path, template: Identity, count: usize) {
        let mut record = Record {
            identity: template,
            tags: TrackTags::default(),
        };
        let mut bytes = HEADER.as_bytes().to_vec();
        for offset in 0..count {
            // Identities no file in the root has.
            record.identity.inode = u64::MAX - offset as u64;
            serde_json::to_writer(&mut bytes, &record).expect("record JSON");
            bytes.push(b'\n');
        }
        fs::write(path, bytes).expect("seed cache");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("private cache");
    }

    fn saved_identities(path: &std::path::Path) -> Vec<Identity> {
        let cache = CachedMetadataReader::read_records(File::open(path).expect("cache"))
            .expect("valid saved cache");
        cache.entries.into_keys().collect()
    }

    #[test]
    fn a_full_cache_of_stale_records_still_admits_fresh_ones() {
        let (temp, root, file) = root();
        let path = temp.path().join("state/metadata.jsonl");
        let identity = Identity::read(&file).expect("identity");
        seed_stale_cache(&path, identity, MAX_ENTRIES);

        let mut parser = TagsReader {
            attempts: 0,
            title: "fresh",
        };
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        assert_eq!(
            cache.entries.len(),
            MAX_ENTRIES,
            "the full stale cache loaded"
        );
        cache
            .read_with(&file, &mut parser)
            .expect("parse the real track");
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("save the next cache");

        assert_eq!(saved_identities(&path), [identity]);
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache.read_with(&file, &mut parser).expect("cache hit");
        assert_eq!(
            parser.attempts, 1,
            "the second start reuses the fresh record"
        );
    }

    #[test]
    fn an_unchanged_cache_is_not_rewritten() {
        use std::os::unix::fs::MetadataExt as _;

        let (temp, root, file) = root();
        let path = temp.path().join("state/metadata.jsonl");
        let mut parser = TagsReader {
            attempts: 0,
            title: "steady",
        };
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache.read_with(&file, &mut parser).expect("first parse");
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("first save");
        // Saving replaces the file by rename, which gives it a new inode.
        let written = fs::metadata(&path).expect("saved cache").ino();

        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        cache.read_with(&file, &mut parser).expect("cache hit");
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("unchanged save");

        assert_eq!(fs::metadata(&path).expect("cache").ino(), written);
        assert_eq!(parser.attempts, 1);
    }

    #[test]
    fn records_that_do_not_fit_on_disk_do_not_trigger_a_rewrite() {
        let (temp, root, file) = root();
        let path = temp.path().join("state/metadata.jsonl");
        let template = Identity::read(&file).expect("identity");
        let tags = TrackTags {
            title: Some("\u{0001}".repeat(5_000)),
            album: Some("\u{0001}".repeat(5_000)),
            ..TrackTags::default()
        };
        // Each record takes 10 KB of tag text and over 60 KB of JSON.
        let identities = (0..650_u32).map(|inode| Identity {
            inode: u64::from(inode),
            ..template
        });
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        for identity in identities.clone() {
            cache.insert_parsed(identity, tags.clone());
        }
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("first save");
        let original = File::open(&path).expect("open original cache");
        let mut cache = CachedMetadataReader::load(root.as_fd(), temp.path());
        assert!(!cache.entries.is_empty());
        assert!(cache.entries.len() < identities.len());

        for identity in identities {
            if cache.hit(&identity).is_none() {
                cache.insert_parsed(identity, tags.clone());
            }
        }
        cache
            .save_inner(root.as_fd(), temp.path())
            .expect("second save");
        assert_eq!(
            fs::metadata(&path).expect("cache").ino(),
            original.metadata().expect("original cache").ino()
        );
    }

    #[test]
    fn the_cache_entry_limit_matches_the_scanner() {
        let scan = crate::config::Config::default().scan;
        assert_eq!(MAX_ENTRIES, scan.max_files);
        assert!(MAX_TAG_BYTES <= scan.max_total_metadata_bytes);
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
        let mut inode = 0_u64;
        while bytes.len() < MAX_CACHE_BYTES {
            record.identity.inode = inode;
            inode += 1;
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
