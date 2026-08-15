//! Content-addressed store: hash, put, get, verify, disk cache.
//!
//! Spec: `docs/specs/asset-cas.md`. Decision: ADR 0006.
//!
//! Identical on the server and on every node, so the same verification pass
//! works in both places. (The server's copy is `server/cas.js`, and
//! `server/content.test.js` asserts the two agree on the hash — they must, or
//! content addressing means nothing across the boundary.)
//!
//! # Rules
//!
//! - **Writes are atomic**: stream to `tmp/`, hash while writing, verify, then
//!   `rename(2)` into `blobs/`. A partial file is never visible as complete.
//! - **Every blob is verified against its hash after download.** Something
//!   corrupting data in transit or at rest should be loud, not logged.
//! - **The cache is permanent by default, not LRU.** Disk is cheap; a
//!   re-download on the morning of a service is not. GC is an explicit operator
//!   action that reports before it removes, and **never runs automatically on
//!   low disk** — a node that silently deletes an asset it needs in an hour has
//!   turned a disk warning into an on-air failure.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Two-level fan-out on the first two bytes of the hash, so a directory never
/// holds a pathological number of entries.
pub const FANOUT_DEPTH: usize = 2;

const PREFIX: &str = "blake3:";

#[derive(Debug, thiserror::Error)]
pub enum CasError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0} is not a content address")]
    NotAnAddress(String),
    #[error("content address mismatch: expected {expected}, got {actual}")]
    Mismatch { expected: String, actual: String },
}

pub fn is_address(h: &str) -> bool {
    h.len() == PREFIX.len() + 64
        && h.starts_with(PREFIX)
        && h[PREFIX.len()..]
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

pub fn address_of(bytes: &[u8]) -> String {
    format!("{PREFIX}{}", blake3::hash(bytes).to_hex())
}

/// A local content-addressed store rooted at a directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, CasError> {
        let root = root.into();
        std::fs::create_dir_all(root.join("blobs"))?;
        std::fs::create_dir_all(root.join("tmp"))?;
        Ok(Self { root })
    }

    pub fn path_of(&self, hash: &str) -> Result<PathBuf, CasError> {
        if !is_address(hash) {
            return Err(CasError::NotAnAddress(hash.to_string()));
        }
        let hex = &hash[PREFIX.len()..];
        Ok(self
            .root
            .join("blobs")
            .join(&hex[0..2])
            .join(&hex[2..4])
            .join(hex))
    }

    pub fn has(&self, hash: &str) -> bool {
        self.path_of(hash).map(|p| p.exists()).unwrap_or(false)
    }

    pub fn get(&self, hash: &str) -> Result<Vec<u8>, CasError> {
        Ok(std::fs::read(self.path_of(hash)?)?)
    }

    /// Store bytes under their own address.
    pub fn put(&self, bytes: &[u8]) -> Result<String, CasError> {
        let hash = address_of(bytes);
        self.put_verified(&hash, bytes)?;
        Ok(hash)
    }

    /// Store bytes that are *claimed* to be `expected`, verifying the claim.
    ///
    /// The claim is checked, never trusted. A store that takes the address on
    /// faith gives up the only guarantee it exists to provide.
    pub fn put_verified(&self, expected: &str, bytes: &[u8]) -> Result<(), CasError> {
        let actual = address_of(bytes);
        if actual != expected {
            return Err(CasError::Mismatch {
                expected: expected.to_string(),
                actual,
            });
        }
        let dest = self.path_of(expected)?;
        // Already present is success. That is the whole point of addressing by
        // content, and re-writing identical bytes would only risk a torn file.
        if dest.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(dest.parent().expect("blob path has a parent"))?;

        // Atomic: write to tmp, then rename. Rename within a filesystem is
        // atomic, so a partial file is never visible as complete — which
        // matters most exactly when it is hardest to notice, on an interrupted
        // download the morning of a service.
        let tmp = self.root.join("tmp").join(&expected[PREFIX.len()..]);
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &dest)?;
        Ok(())
    }

    /// Stream a reader into the store, hashing as it goes.
    ///
    /// Never buffers a whole file in memory: a 4 GB video must not need 4 GB of
    /// RAM to arrive.
    pub fn put_stream(&self, expected: &str, mut src: impl Read) -> Result<u64, CasError> {
        if !is_address(expected) {
            return Err(CasError::NotAnAddress(expected.to_string()));
        }
        let tmp = self.root.join("tmp").join(&expected[PREFIX.len()..]);
        let mut hasher = blake3::Hasher::new();
        let mut written = 0u64;
        {
            let mut f = std::fs::File::create(&tmp)?;
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                f.write_all(&buf[..n])?;
                written += n as u64;
            }
            f.sync_all()?;
        }
        let actual = format!("{PREFIX}{}", hasher.finalize().to_hex());
        if actual != expected {
            // Remove the impostor rather than leaving it in tmp to be mistaken
            // for a resumable partial.
            let _ = std::fs::remove_file(&tmp);
            return Err(CasError::Mismatch {
                expected: expected.to_string(),
                actual,
            });
        }
        let dest = self.path_of(expected)?;
        std::fs::create_dir_all(dest.parent().expect("blob path has a parent"))?;
        std::fs::rename(&tmp, &dest)?;
        Ok(written)
    }

    /// Re-hash every blob. Returns `(checked, mismatched)`.
    pub fn verify(&self) -> Result<(usize, Vec<String>), CasError> {
        let mut checked = 0;
        let mut bad = Vec::new();
        let blobs = self.root.join("blobs");
        for a in read_dir(&blobs)? {
            for b in read_dir(&a)? {
                for f in read_dir(&b)? {
                    checked += 1;
                    let name = f.file_name().and_then(|s| s.to_str()).unwrap_or_default();
                    let bytes = std::fs::read(&f)?;
                    if address_of(&bytes) != format!("{PREFIX}{name}") {
                        bad.push(format!("{PREFIX}{name}"));
                    }
                }
            }
        }
        Ok((checked, bad))
    }
}

fn read_dir(p: &Path) -> Result<Vec<PathBuf>, CasError> {
    match std::fs::read_dir(p) {
        Ok(it) => Ok(it.filter_map(|e| e.ok().map(|e| e.path())).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}

pub fn selftest() -> Result<(), String> {
    // The known blake3 vector, so a bad dependency swap is caught here and not
    // by two systems quietly disagreeing about what a file is called.
    if address_of(b"hello")
        != "blake3:ea8f163db38682925e4491c5e58d4bb3506ef8c14eb78a86e908c5624a67200f"
    {
        return Err("blake3 does not match the published test vector".into());
    }
    Ok(())
}
