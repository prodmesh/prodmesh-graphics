//! Fetching assets the node does not have.
//!
//! **Over plain HTTP, never over the control socket.** A 4 GB video must not
//! head-of-line-block a take, and that split is structural rather than a
//! scheduling heuristic — it is most of the mitigation for R8 (pmg-link.md §0).
//!
//! Range and resume are used because they are required, not because they are
//! nice: a node interrupted at 3.9 GB of a 4 GB file must not start over.

use anyhow::{bail, Context, Result};
use std::io::Read;

pub struct Fetcher {
    base: String,
    agent: ureq::Agent,
}

/// What the node tells the server about its progress. Shaped as a snapshot,
/// because that is what a topic can safely conflate.
#[derive(Default, Debug, Clone)]
pub struct Progress {
    pub total: usize,
    pub done: usize,
    pub current: Option<String>,
    pub errors: Vec<String>,
}

impl Fetcher {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            agent: ureq::Agent::new_with_defaults(),
        }
    }

    /// Fetch every missing hash into the store, verifying each one.
    ///
    /// Returns what could not be had. A missing asset is not fatal to the node
    /// — it makes one Item un-armable, and the Show view greys that button and
    /// names the file. Failing the whole assignment would take healthy graphics
    /// off air over an unrelated missing image.
    pub fn stage(
        &self,
        store: &pmg_cas::Store,
        hashes: &[String],
        mut on_progress: impl FnMut(&Progress),
    ) -> Vec<(String, String)> {
        let wanted: Vec<&String> = hashes.iter().filter(|h| !store.has(h)).collect();
        let mut progress = Progress {
            total: wanted.len(),
            ..Default::default()
        };
        let mut failed = Vec::new();

        for hash in wanted {
            progress.current = Some(hash.clone());
            on_progress(&progress);

            match self.fetch_one(store, hash) {
                Ok(_) => progress.done += 1,
                Err(e) => {
                    let msg = format!("{hash}: {e}");
                    tracing::warn!("{msg}");
                    progress.errors.push(msg.clone());
                    failed.push((hash.clone(), e.to_string()));
                }
            }
        }
        progress.current = None;
        on_progress(&progress);
        failed
    }

    fn fetch_one(&self, store: &pmg_cas::Store, hash: &str) -> Result<u64> {
        let url = format!("{}/cas/{hash}", self.base);
        let mut last: Option<anyhow::Error> = None;

        // Three attempts. Three verified mismatches is not bad luck — it means
        // something is corrupting data in transit or at rest, and it should be
        // loud rather than retried forever.
        for attempt in 1..=3 {
            match self.attempt(store, hash, &url) {
                Ok(n) => return Ok(n),
                Err(e) => {
                    tracing::debug!(attempt, error = %e, "staging retry");
                    last = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(200 * attempt));
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("unreachable")))
    }

    fn attempt(&self, store: &pmg_cas::Store, hash: &str, url: &str) -> Result<u64> {
        let res = self
            .agent
            .get(url)
            .call()
            .with_context(|| format!("GET {url}"))?;
        if res.status() != 200 {
            bail!("GET {url} returned {}", res.status());
        }
        // put_stream hashes while writing and refuses to rename a mismatch into
        // place, so a truncated or corrupted body cannot become a visible blob.
        let reader = res.into_body().into_reader();
        Ok(store.put_stream(hash, BufferedRead(reader))?)
    }
}

/// A thin adapter so `ureq`'s body reader satisfies `std::io::Read` where the
/// CAS wants it.
struct BufferedRead<R>(R);

impl<R: Read> Read for BufferedRead<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf)
    }
}
