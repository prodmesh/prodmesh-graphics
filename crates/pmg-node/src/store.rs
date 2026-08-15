//! The node's disk: its identity, its assignment, and its CAS.
//!
//! # This module is where the autonomy invariant is actually kept
//!
//! ADR 0009 says a node keeps playing when the server dies. That is not a
//! property of the link code — it is a property of *what is on disk before the
//! link is acked*. `save_assignment` returns only once the bytes are durably
//! written, and `link.rs` sends `AssignmentAck` only after it returns. Reverse
//! those two and the invariant is gone with no visible symptom until the day it
//! matters.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The node's own identity on disk.
pub struct Identity {
    /// Stable across restarts. Generated once, then never again — this is how
    /// the server recognises the same physical node.
    pub node_id: String,
    /// **New every process start.** With the server's `epoch` this is what
    /// separates "the node restarted" from "the server restarted", which need
    /// opposite responses (pmg-link.md §2.2).
    pub boot_id: String,
    pub name: String,
}

pub struct Store {
    pub dir: PathBuf,
    pub cas: pmg_cas::Store,
}

impl Store {
    pub fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let cas = pmg_cas::Store::open(dir.join("cas"))?;
        Ok(Self { dir, cas })
    }

    pub fn identity(&self, name: Option<String>) -> Result<Identity> {
        let path = self.dir.join("node_id");
        let node_id = match std::fs::read_to_string(&path) {
            Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                let id = random_id();
                std::fs::write(&path, &id)?;
                tracing::info!(node_id = %id, "generated a node identity");
                id
            }
        };
        Ok(Identity {
            name: name
                .unwrap_or_else(|| hostname().unwrap_or_else(|| format!("node-{}", &node_id[..6]))),
            node_id,
            boot_id: random_id(),
        })
    }

    fn assignment_path(&self) -> PathBuf {
        self.dir.join("assignment.cbor")
    }

    /// Read the assignment this node is holding, if any.
    ///
    /// A corrupt file is treated as *no assignment* rather than as a fatal
    /// error: coming up with nothing and asking the server is recoverable,
    /// while refusing to boot is not.
    pub fn load_assignment(&self) -> Option<pmg_proto::Assignment> {
        let bytes = std::fs::read(self.assignment_path()).ok()?;
        match pmg_proto::decode::<pmg_proto::Assignment>(&bytes) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!(error = %e, "assignment.cbor is unreadable; starting with none");
                None
            }
        }
    }

    /// Write the assignment durably.
    ///
    /// **Returns only once it would survive losing power.** Write to a temp
    /// file, fsync the file, rename, then fsync the *directory* — without that
    /// last step the rename itself can be lost on some filesystems, which is
    /// the failure that looks like "it acked and then forgot".
    pub fn save_assignment(&self, a: &pmg_proto::Assignment) -> Result<()> {
        let bytes = pmg_proto::encode(a).map_err(|e| anyhow::anyhow!("{e}"))?;
        let dest = self.assignment_path();
        let tmp = dest.with_extension("cbor.tmp");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &dest)?;
        if let Ok(d) = std::fs::File::open(&self.dir) {
            let _ = d.sync_all();
        }
        Ok(())
    }
}

fn hostname() -> Option<String> {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().trim_end_matches(".local").to_string())
        .filter(|s| !s.is_empty())
}

/// A random identifier. Not a content address, and deliberately shaped
/// differently from one so the two are never confused at a glance.
fn random_id() -> String {
    // Hash of time plus the process id: no rand dependency for something that
    // needs to be unique rather than unpredictable.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let seed = format!("{t}-{}", std::process::id());
    blake3::hash(seed.as_bytes()).to_hex()[..26].to_uppercase()
}
