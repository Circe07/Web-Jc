//! Persistent per-file content-hash index.
//!
//! For every file that feeds the cache key the database keeps
//! `(size, mtime_ns, ctime_ns, inode, blake3(contents))`, keyed by a short id
//! of the workspace root plus the relative path. A stored hash is reused only
//! if size, mtime, ctime and inode (the last two where the platform has them)
//! are all unchanged and the mtime is more than [`RACY_WINDOW_NS`] older than
//! now. Entries are also only *stored* when that holds at hashing time, which
//! is the "racy git" guard: a file written less than 2 s ago is always
//! re-read, since a later same-timestamp write could otherwise go unnoticed.
//!
//! Known limitation: on platforms without ctime (non-unix), an edit that
//! keeps the file size and then resets the mtime to its old value is not
//! detected. On unix the ctime changes on such a reset, so it is detected.

use std::fs::Metadata;
use std::time::{SystemTime, UNIX_EPOCH};

/// Files modified within this window before "now" are never trusted.
pub(crate) const RACY_WINDOW_NS: i128 = 2_000_000_000;

/// Cheap-to-read identity of a file's on-disk state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Stamp {
    pub size: u64,
    pub mtime_ns: i128,
    pub ctime_ns: i128,
    pub inode: u64,
}

/// A stored index row.
pub(crate) struct Entry {
    pub stamp: Stamp,
    pub hash: [u8; 32],
}

const ENCODED_LEN: usize = 8 + 16 + 16 + 8 + 32;

fn system_ns(t: SystemTime) -> i128 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
        Err(e) => -i128::try_from(e.duration().as_nanos()).unwrap_or(i128::MAX),
    }
}

/// Current time in ns since the epoch.
pub(crate) fn now_ns() -> i128 {
    system_ns(SystemTime::now())
}

impl Stamp {
    /// Reads the stamp from file metadata.
    pub(crate) fn of(meta: &Metadata) -> Self {
        let mtime_ns = meta.modified().map_or(0, system_ns);
        #[cfg(unix)]
        let (ctime_ns, inode) = {
            use std::os::unix::fs::MetadataExt;
            (
                i128::from(meta.ctime()) * 1_000_000_000 + i128::from(meta.ctime_nsec()),
                meta.ino(),
            )
        };
        #[cfg(not(unix))]
        let (ctime_ns, inode) = (0, 0);
        Self {
            size: meta.len(),
            mtime_ns,
            ctime_ns,
            inode,
        }
    }

    /// `true` if the mtime is old enough (relative to `now`) to be trusted.
    pub(crate) fn settled(&self, now: i128) -> bool {
        self.mtime_ns <= now - RACY_WINDOW_NS
    }
}

impl Entry {
    /// Serializes the row.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(ENCODED_LEN);
        v.extend_from_slice(&self.stamp.size.to_le_bytes());
        v.extend_from_slice(&self.stamp.mtime_ns.to_le_bytes());
        v.extend_from_slice(&self.stamp.ctime_ns.to_le_bytes());
        v.extend_from_slice(&self.stamp.inode.to_le_bytes());
        v.extend_from_slice(&self.hash);
        v
    }

    /// Parses a row; `None` if malformed.
    pub(crate) fn decode(b: &[u8]) -> Option<Self> {
        if b.len() != ENCODED_LEN {
            return None;
        }
        Some(Self {
            stamp: Stamp {
                size: u64::from_le_bytes(b[0..8].try_into().ok()?),
                mtime_ns: i128::from_le_bytes(b[8..24].try_into().ok()?),
                ctime_ns: i128::from_le_bytes(b[24..40].try_into().ok()?),
                inode: u64::from_le_bytes(b[40..48].try_into().ok()?),
            },
            hash: b[48..80].try_into().ok()?,
        })
    }
}
