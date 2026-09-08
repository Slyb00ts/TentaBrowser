//! Cache persistence (`docs/spec/03-network.md` §5): an append-only log with
//! an index rebuilt by scanning, and no SQLite dependency.
//!
//! The directory is the caller's `BrowserContext::cache_dir`, which the host
//! is expected to have run through `FsPolicy` before handing it here
//! (`docs/spec/19-security.md` §1). Nothing in this module widens a policy.

use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use bytes::Bytes;
use hashbrown::HashMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tenta_core::{Error, Result};

/// The log file inside the cache directory.
const SEGMENT: &str = "cache.log";
/// Record kinds in the log.
const RECORD: u8 = b'R';
const TOMBSTONE: u8 = b'T';
/// Refuse to allocate for a length that cannot be a real record; a corrupt
/// header would otherwise ask for gigabytes.
const MAX_RECORD_BYTES: u64 = 1 << 30;

/// One stored response as it appears on disk and in the index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheRecord {
    /// Primary key: method and URL.
    pub key: String,
    /// Which `Vary` variant of the key this is.
    pub variant: u64,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// The request header values this variant was selected by.
    pub varied: Vec<(String, Option<String>)>,
    pub vary_star: bool,
    pub request_time: u64,
    pub response_time: u64,
    pub last_access: u64,
    /// Carried in memory; on disk it follows the JSON header.
    #[serde(skip)]
    pub body: Bytes,
}

impl CacheRecord {
    fn id(&self) -> (String, u64) {
        (self.key.clone(), self.variant)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tombstone {
    key: String,
    variant: u64,
}

/// Durability for [`super::HttpCache`].
///
/// The cache keeps its index in memory either way; a store only decides
/// whether that index survives the process.
pub trait CacheStore: Send + Sync + fmt::Debug {
    /// Every live record, called once when the cache is constructed.
    fn load(&self) -> Result<Vec<CacheRecord>>;
    fn append(&self, record: &CacheRecord) -> Result<()>;
    fn remove(&self, key: &str, variant: u64) -> Result<()>;
    /// Whether dead bytes have grown past the compaction threshold.
    fn needs_compaction(&self) -> bool;
    /// Rewrites the log with only `live` in it.
    fn compact(&self, live: &[CacheRecord]) -> Result<()>;
    /// Bytes currently occupied on disk; zero for a memory-only store.
    fn disk_bytes(&self) -> u64;
    /// Whether `append`/`compact` touch the filesystem, in which case the
    /// caller must not run them on an async runtime thread.
    fn writes_block(&self) -> bool {
        true
    }
}

/// Memory-only durability: the cache's own index is the storage, so every
/// operation here is a no-op and nothing survives the process.
#[derive(Debug, Default, Clone, Copy)]
pub struct MemoryStore;

impl CacheStore for MemoryStore {
    fn writes_block(&self) -> bool {
        false
    }

    fn load(&self) -> Result<Vec<CacheRecord>> {
        Ok(Vec::new())
    }

    fn append(&self, _record: &CacheRecord) -> Result<()> {
        Ok(())
    }

    fn remove(&self, _key: &str, _variant: u64) -> Result<()> {
        Ok(())
    }

    fn needs_compaction(&self) -> bool {
        false
    }

    fn compact(&self, _live: &[CacheRecord]) -> Result<()> {
        Ok(())
    }

    fn disk_bytes(&self) -> u64 {
        0
    }
}

#[derive(Debug)]
struct DiskInner {
    file: File,
    /// On-disk size of the newest record of each id, for dead-byte accounting.
    sizes: HashMap<(String, u64), u64>,
    live_bytes: u64,
    dead_bytes: u64,
}

/// An append-only log with an index rebuilt by scanning on open.
#[derive(Debug)]
pub struct DiskStore {
    dir: PathBuf,
    path: PathBuf,
    inner: Mutex<DiskInner>,
    /// Compact once dead bytes exceed this share of the file.
    compaction_ratio: f64,
}

impl DiskStore {
    /// Opens (creating if needed) the log under `dir`.
    ///
    /// A record torn by a crash is dropped and the file is truncated to the
    /// last complete record, which is why the format is length-prefixed.
    pub fn open(dir: impl Into<PathBuf>) -> Result<DiskStore> {
        let dir = dir.into();
        crate::policy::private_files::create_private_dir(&dir)?;
        let path = dir.join(SEGMENT);
        let file = crate::policy::private_files::private_file()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        // A log written before this rule existed keeps its old mode otherwise.
        crate::policy::private_files::harden_file(&path)?;
        let store = DiskStore {
            dir,
            path,
            inner: Mutex::new(DiskInner {
                file,
                sizes: HashMap::new(),
                live_bytes: 0,
                dead_bytes: 0,
            }),
            compaction_ratio: 0.5,
        };
        store.rebuild_index()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn dead_bytes(&self) -> u64 {
        self.inner.lock().dead_bytes
    }

    pub fn live_bytes(&self) -> u64 {
        self.inner.lock().live_bytes
    }

    fn rebuild_index(&self) -> Result<()> {
        let mut inner = self.inner.lock();
        inner.sizes.clear();
        inner.live_bytes = 0;
        inner.dead_bytes = 0;

        let mut file = File::open(&self.path)?;
        let mut offset = 0u64;
        while let Some((entry, size)) = read_record(&mut file)? {
            match entry {
                Entry::Record(record) => {
                    let id = record.id();
                    if let Some(previous) = inner.sizes.insert(id, size) {
                        inner.live_bytes -= previous;
                        inner.dead_bytes += previous;
                    }
                    inner.live_bytes += size;
                }
                Entry::Tombstone(tombstone) => {
                    if let Some(previous) = inner.sizes.remove(&(tombstone.key, tombstone.variant))
                    {
                        inner.live_bytes -= previous;
                        inner.dead_bytes += previous;
                    }
                    inner.dead_bytes += size;
                }
            }
            offset += size;
        }

        // Anything after the last complete record is a torn write.
        let length = file.metadata()?.len();
        if length > offset {
            drop(file);
            let handle = OpenOptions::new().write(true).open(&self.path)?;
            handle.set_len(offset)?;
            handle.sync_all()?;
            let mut reopened = OpenOptions::new()
                .create(true)
                .read(true)
                .append(true)
                .open(&self.path)?;
            reopened.seek(SeekFrom::End(0))?;
            inner.file = reopened;
        }
        Ok(())
    }

    /// Every live record, in log order.
    fn scan(&self) -> Result<Vec<CacheRecord>> {
        let mut file = File::open(&self.path)?;
        let mut live: Vec<(String, u64)> = Vec::new();
        let mut records: HashMap<(String, u64), CacheRecord> = HashMap::new();
        while let Some((entry, _)) = read_record(&mut file)? {
            match entry {
                Entry::Record(record) => {
                    let id = record.id();
                    if !records.contains_key(&id) {
                        live.push(id.clone());
                    }
                    records.insert(id, record);
                }
                Entry::Tombstone(tombstone) => {
                    let id = (tombstone.key, tombstone.variant);
                    records.remove(&id);
                    live.retain(|candidate| *candidate != id);
                }
            }
        }
        Ok(live
            .into_iter()
            .filter_map(|id| records.remove(&id))
            .collect())
    }
}

impl CacheStore for DiskStore {
    fn load(&self) -> Result<Vec<CacheRecord>> {
        self.scan()
    }

    fn append(&self, record: &CacheRecord) -> Result<()> {
        let bytes = encode_record(record)?;
        let mut inner = self.inner.lock();
        inner.file.write_all(&bytes)?;
        inner.file.flush()?;
        let size = bytes.len() as u64;
        if let Some(previous) = inner.sizes.insert(record.id(), size) {
            inner.live_bytes -= previous;
            inner.dead_bytes += previous;
        }
        inner.live_bytes += size;
        Ok(())
    }

    fn remove(&self, key: &str, variant: u64) -> Result<()> {
        let bytes = encode_tombstone(key, variant)?;
        let mut inner = self.inner.lock();
        if let Some(previous) = inner.sizes.remove(&(key.to_string(), variant)) {
            inner.live_bytes -= previous;
            inner.dead_bytes += previous;
        } else {
            // Nothing on disk to erase, so no tombstone is needed.
            return Ok(());
        }
        inner.file.write_all(&bytes)?;
        inner.file.flush()?;
        inner.dead_bytes += bytes.len() as u64;
        Ok(())
    }

    fn needs_compaction(&self) -> bool {
        let inner = self.inner.lock();
        let total = inner.live_bytes + inner.dead_bytes;
        total > 0 && (inner.dead_bytes as f64) > (total as f64) * self.compaction_ratio
    }

    fn compact(&self, live: &[CacheRecord]) -> Result<()> {
        let mut sizes = HashMap::new();
        let mut live_bytes = 0u64;
        let mut encoded = Vec::new();
        for record in live {
            let bytes = encode_record(record)?;
            sizes.insert(record.id(), bytes.len() as u64);
            live_bytes += bytes.len() as u64;
            encoded.extend_from_slice(&bytes);
        }

        let mut inner = self.inner.lock();
        // Rewritten in place rather than through a temp file and a rename:
        // Windows refuses to replace a file that is still open, and a torn
        // rewrite of this log is recoverable because a scan stops at the
        // first incomplete record.
        {
            let file = OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&self.path)?;
            let mut writer = BufWriter::new(file);
            writer.write_all(&encoded)?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
        }
        inner.file = crate::policy::private_files::private_file()
            .create(true)
            .read(true)
            .append(true)
            .open(&self.path)?;
        inner.sizes = sizes;
        inner.live_bytes = live_bytes;
        inner.dead_bytes = 0;
        Ok(())
    }

    fn disk_bytes(&self) -> u64 {
        let inner = self.inner.lock();
        inner.live_bytes + inner.dead_bytes
    }
}

enum Entry {
    Record(CacheRecord),
    Tombstone(Tombstone),
}

/// `kind | u32 header length | u32 body length | header JSON | body`.
fn encode_record(record: &CacheRecord) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(record).map_err(encode_error)?;
    let mut out = Vec::with_capacity(9 + json.len() + record.body.len());
    out.push(RECORD);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&(record.body.len() as u32).to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&record.body);
    Ok(out)
}

fn encode_tombstone(key: &str, variant: u64) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(&Tombstone {
        key: key.to_string(),
        variant,
    })
    .map_err(encode_error)?;
    let mut out = Vec::with_capacity(9 + json.len());
    out.push(TOMBSTONE);
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&json);
    Ok(out)
}

fn encode_error(error: serde_json::Error) -> Error {
    Error::Other {
        code: "http_cache_encode".into(),
        message: error.to_string(),
    }
}

/// Reads one record. `Ok(None)` means a clean end of file or a torn record,
/// both of which stop the scan.
fn read_record(file: &mut File) -> Result<Option<(Entry, u64)>> {
    let mut header = [0u8; 9];
    if !read_exact_or_eof(file, &mut header)? {
        return Ok(None);
    }
    let kind = header[0];
    let json_len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as u64;
    let body_len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as u64;
    if (kind != RECORD && kind != TOMBSTONE)
        || json_len > MAX_RECORD_BYTES
        || body_len > MAX_RECORD_BYTES
    {
        return Ok(None);
    }

    let mut json = vec![0u8; json_len as usize];
    if !read_exact_or_eof(file, &mut json)? {
        return Ok(None);
    }
    let mut body = vec![0u8; body_len as usize];
    if !read_exact_or_eof(file, &mut body)? {
        return Ok(None);
    }

    let size = 9 + json_len + body_len;
    match kind {
        RECORD => {
            let Ok(mut record) = serde_json::from_slice::<CacheRecord>(&json) else {
                return Ok(None);
            };
            record.body = Bytes::from(body);
            Ok(Some((Entry::Record(record), size)))
        }
        _ => {
            let Ok(tombstone) = serde_json::from_slice::<Tombstone>(&json) else {
                return Ok(None);
            };
            Ok(Some((Entry::Tombstone(tombstone), size)))
        }
    }
}

/// `Ok(false)` when the buffer could not be filled, which is the torn-record
/// case rather than an error.
fn read_exact_or_eof(file: &mut File, buffer: &mut [u8]) -> Result<bool> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => return Ok(false),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(Error::Io(error)),
        }
    }
    Ok(true)
}
