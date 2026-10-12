//! The node runtime: owns the layer engine, drives the frame loop.
//!
//! # Who is authoritative for what
//!
//! This process is authoritative for **what is on air**. The server is
//! authoritative for **content and assignment**. Every message that arrives is
//! either content (accept it, store it) or a cue (apply it, ack it) — and
//! nothing the server sends is allowed to *state* what is currently showing.
//! That is what makes a server restart a non-event (ADR 0009).
//!
//! # Boot behaviour
//!
//! It comes up **black**, with a valid assignment loaded but nothing on air.
//! Auto-restore is deliberately not implemented: an unattended machine
//! re-firing a graphic after a power cut is a bad surprise, and it must be an
//! explicit per-output opt-in when it exists at all.

use anyhow::{Context, Result};
use pmg_proto::DataValue;
use pmg_scene::doc::SceneDoc;
use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::engine::{self, LayerEngine, Library};
use crate::link::{FromLink, ToLink};
use crate::render::{Gpu, Target};
use crate::staging::Fetcher;
use crate::store::Store;

/// Everything the engine needs to arm an item, assembled from the assignment
/// plus the local CAS.
pub struct Assigned {
    pub templates: BTreeMap<String, SceneDoc>,
    pub items: BTreeMap<String, (String, String, BTreeMap<String, DataValue>)>,
    pub font_bytes: Vec<Vec<u8>>,
    /// Decoded stills, keyed by the asset KEY the document uses (not the hash).
    /// Decoded once at assignment; arming a hundred items must not decode the
    /// same background a hundred times.
    pub images: BTreeMap<String, (u32, u32, std::sync::Arc<Vec<u8>>)>,
    /// Clips, by asset key: where the file is and what it is.
    ///
    /// A path and a spec, never pixels. The poster frame goes into `images` so
    /// the scene builder can size the texture and an armed-but-unplayed clip has
    /// something honest to show.
    pub videos: BTreeMap<String, (std::path::PathBuf, pmg_media::video::VideoSpec)>,
    /// A developer-supplied face, used only when the assignment carried no
    /// usable font. Named `--font` on the command line, and it exists so the
    /// slice can be driven before font assets are wired end to end.
    pub fallback_font: Option<Vec<u8>>,
}

impl Library for Assigned {
    fn template(&self, template_rev: &str) -> Option<&SceneDoc> {
        self.templates.get(template_rev)
    }

    fn item(&self, item_id: &str) -> Option<(&str, &str, &BTreeMap<String, DataValue>)> {
        self.items
            .get(item_id)
            .map(|(label, rev, data)| (label.as_str(), rev.as_str(), data))
    }

    fn image(&self, asset_key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
        self.images.get(asset_key).cloned()
    }

    fn video(&self, asset_key: &str) -> Option<(std::path::PathBuf, pmg_media::video::VideoSpec)> {
        self.videos.get(asset_key).cloned()
    }

    fn text_engine(&self) -> Result<pmg_text::TextEngine> {
        let faces: Vec<Vec<u8>> = if self.font_bytes.is_empty() {
            self.fallback_font.iter().cloned().collect()
        } else {
            self.font_bytes.clone()
        };
        let mut families: Vec<(String, u16)> = self
            .templates
            .values()
            .flat_map(|d| d.fonts.iter().map(|f| (f.family.clone(), f.weight)))
            .collect();
        families.sort();
        families.dedup();
        if families.is_empty() {
            families.push(("Default".into(), 400));
        }
        // Faces register under their OWN names, with the every-face-per-family
        // shim only for families no face answered to (which is also how the
        // single `--font` stand-in keeps working). The rule is SHARED with the
        // designer (pmg-text): registering every face under every family here
        // made two staged fonts collapse to one on air, while the preview
        // showed both — the disagreement ADR 0001 exists to prevent.
        let db = pmg_text::build_font_db(&faces, &families)?;
        Ok(pmg_text::TextEngine::new(db))
    }
}

/// Where an output's frames go.
///
/// One enum rather than a trait object because M2 has exactly two backends and
/// they differ in how they are PACED — NDI clocks itself, DeckLink schedules
/// against its own clock. A trait would hide that difference behind a uniform
/// `send()`, which is the one thing about them that is not uniform.
#[allow(clippy::large_enum_variant)]
pub enum Backend {
    /// Render and time frames, send nothing.
    Headless,
    // Boxed would be one indirection on every frame's send for the sake of a
    // variant that exists once per output. There are one to four of these in a
    // process, not a collection of them.
    Ndi(pmg_out_ndi::Sender),
    DeckLink(Box<pmg_out_decklink::Sender>),
}

/// What this node can say about one Item, destination by destination.
///
/// Free and pure so it can be tested without a CAS, a GPU, or a link — the
/// interesting cases are an Item at two revisions where only one is staged, and
/// that has nothing to do with any of them.
///
/// `staged` answers whether a revision loaded; `rev_problem` says why one did
/// not, and distinguishes a missing byte (`None`, resolves itself by waiting)
/// from a version refusal (`Some`, never will).
pub fn readiness_of(
    it: &pmg_proto::ItemRef,
    staged: impl Fn(&str) -> bool,
    rev_problem: &std::collections::HashMap<String, Option<String>>,
) -> pmg_proto::ItemReadiness {
    // An Item with no destination list predates ADR 0018, or came from a server
    // that does. Its own output/layer/template_rev ARE the one destination, so
    // it becomes a list of one and the rest is a single code path — a second
    // branch here is one that would drift out of step with the first.
    let dests: Vec<(String, String, String)> = if it.destinations.is_empty() {
        vec![(
            it.output.clone().unwrap_or_default(),
            it.layer.clone().unwrap_or_default(),
            it.template_rev.clone(),
        )]
    } else {
        it.destinations
            .iter()
            .map(|d| (d.output.clone(), d.layer.clone(), d.template_rev.clone()))
            .collect()
    };

    pmg_proto::ItemReadiness {
        item: it.id.clone(),
        destinations: dests
            .into_iter()
            .map(|(output, layer, rev)| {
                let ok = staged(&rev);
                let unreadable = rev_problem.get(&rev).cloned().flatten();
                pmg_proto::DestinationReadiness {
                    ready: ok,
                    // Empty when it is staged, and ALSO empty when it was
                    // refused: a refused template is not outstanding, it is
                    // decided. Listing it in `missing` puts "still copying"
                    // under a graphic that will never copy — a sentence that
                    // tells an operator to wait through something that has
                    // already failed. `unreadable` is the answer there.
                    missing: if ok || unreadable.is_some() {
                        Vec::new()
                    } else {
                        vec![rev.clone()]
                    },
                    unreadable,
                    output,
                    layer,
                    template_rev: rev,
                }
            })
            .collect(),
    }
}

/// One physical output this node drives.
#[derive(Clone)]
pub struct OutputSpec {
    pub id: String,
    pub layers: Vec<String>,
    pub fps: (u32, u32),
    pub width: u32,
    pub height: u32,
    /// `ndi` | `decklink` | `headless`
    pub backend: String,
    pub ndi_name: String,
    pub decklink_device: usize,
    /// Second port for the key half of a fill+key pair.
    pub decklink_key_device: Option<usize>,
    pub decklink_mode: String,
    /// 0 disables this output's preview channel.
    pub preview_port: u16,
    /// Which **clock** this output runs on. Outputs naming the same group share
    /// a render thread, because that is what "phase-locked together" means in
    /// practice (ADR 0011).
    ///
    /// Two outputs on a genlocked card belong in one group. Two outputs on
    /// unrelated references do not, and pretending otherwise would produce a
    /// thread pacing itself against a clock half its outputs do not follow.
    pub group: String,
    /// Embedded audio channels on this output: 2, 8 or 16, or 0 for none.
    ///
    /// Per OUTPUT rather than per template, because the same graphic played to a
    /// lower-third key and to a wall feed needs its audio in different places or
    /// nowhere (`docs/specs/output-config.md` §7).
    pub audio_channels: i32,
    /// Where those channels GO (ADR 0028). Empty is embedded in the video —
    /// SDI or NDI — which is what every output meant before this existed.
    pub audio_device: String,
    /// The host API that device is on. Empty is the platform's default.
    pub audio_host: String,
    /// Stall this output's render thread once, this many seconds in.
    ///
    /// **A test instrument, not a feature.** A booth machine hiccups for
    /// reasons that leave no trace — the one measured in the two-hour soak had
    /// no warning, no error, and no correlation with anything running. Waiting
    /// for another is not a test, so this makes one on demand and the recovery
    /// becomes something that can be asserted.
    pub inject_stall_at_s: Option<u64>,
    /// Serve the old MJPEG preview instead of H.264.
    ///
    /// Kept because it needs no decoder in the browser at all, which makes it
    /// the instrument that answers "is ANY picture coming out of this node" when
    /// the answer for H.264 is no.
    pub preview_mjpeg: bool,
}

pub struct Options {
    pub outputs: Vec<OutputSpec>,
    pub server_http: String,
    pub fallback_font: Option<Vec<u8>>,
}

pub struct Runtime {
    engine: Arc<Mutex<LayerEngine>>,
    assigned: Assigned,
    store: Store,
    fetcher: Fetcher,
    opts: Options,
}

impl Runtime {
    pub fn new(store: Store, opts: Options) -> Self {
        let fetcher = Fetcher::new(&opts.server_http);
        let mut engine = LayerEngine::new();
        // Seed from the command line so the node is usable before a server has
        // said anything. The assignment overrides this the moment one arrives.
        for o in &opts.outputs {
            engine.configure(
                &[pmg_proto::OutputAssignment {
                    id: o.id.clone(),
                    mode: format!("{}x{}", o.width, o.height),
                    color: Default::default(),
                    layers: o.layers.clone(),
                }],
                o.fps,
            );
        }
        // From here on the assignment may configure these outputs but not add
        // to them: the node knows its own wiring, the server does not.
        engine.seal_outputs();
        Self {
            engine: Arc::new(Mutex::new(engine)),
            assigned: Assigned {
                templates: BTreeMap::new(),
                items: BTreeMap::new(),
                font_bytes: Vec::new(),
                images: BTreeMap::new(),
                videos: BTreeMap::new(),
                fallback_font: opts.fallback_font.clone(),
            },
            store,
            fetcher,
            opts,
        }
    }

    /// Adopt an assignment that has already been persisted.
    ///
    /// Called both on boot (from `assignment.cbor`) and on a fresh offer. The
    /// two paths are identical on purpose: a node that behaves differently
    /// depending on whether the server is present has a second code path that
    /// only runs during an outage, which is the worst possible time to find a
    /// bug in it.
    pub fn adopt(&mut self, a: &pmg_proto::Assignment, tx: Option<&Sender<ToLink>>) {
        {
            let mut e = self.engine.lock().unwrap();
            e.gen = a.gen;
            // Each output keeps the frame rate its own spec named; the
            // assignment says which LAYERS an output has, not what clock it
            // runs on — that is a property of the wiring, not of the content.
            let fps = self
                .opts
                .outputs
                .first()
                .map(|o| o.fps)
                .unwrap_or((60000, 1001));
            e.configure(&a.outputs, fps);
        }

        // Stage what is missing. Over HTTP, never the control socket.
        let missing: Vec<String> = a
            .assets
            .iter()
            .chain(a.templates.iter().map(|t| &t.template_rev))
            .filter(|h| !self.store.has_str(h))
            .cloned()
            .collect();
        if !missing.is_empty() {
            let failed = self.fetcher.stage(&self.store.cas, &missing, |p| {
                if let Some(tx) = tx {
                    let _ = tx.send(ToLink::Progress(pmg_proto::SyncProgress {
                        total_bytes: 0,
                        done_bytes: 0,
                        pending: (p.total - p.done) as u32,
                        current: p.current.clone(),
                        errors: p.errors.clone(),
                    }));
                }
            });
            for (hash, why) in &failed {
                tracing::warn!(%hash, %why, "could not stage");
            }
        }

        // Load templates, refusing anything too new to read HERE, at assignment
        // time — loudly, with both numbers, so you find out Saturday and not at
        // 10:15 Sunday (pmg-link.md §4).
        //
        // Keyed by revision, NOT by item. A revision is what fails; an item
        // merely points at one, and several destinations of one item can point
        // at different ones. The old shape put a `template_rev` in a field
        // called `item`, which read as an item id to everything downstream.
        let mut rev_problem: std::collections::HashMap<String, Option<String>> =
            std::collections::HashMap::new();
        self.assigned.templates.clear();
        for t in &a.templates {
            match self.load_template(&t.template_rev) {
                Ok(doc) if doc.min_reader > pmg_proto::SCENE_READER_MAJOR => {
                    tracing::error!(
                        template = %t.name,
                        needs = doc.min_reader,
                        have = pmg_proto::SCENE_READER_MAJOR,
                        "REFUSING this template: it needs a newer reader than this node has"
                    );
                    rev_problem.insert(
                        t.template_rev.clone(),
                        Some(format!(
                            "needs reader {}, this node is {}",
                            doc.min_reader,
                            pmg_proto::SCENE_READER_MAJOR
                        )),
                    );
                }
                Ok(doc) => {
                    self.assigned.templates.insert(t.template_rev.clone(), doc);
                }
                Err(e) => {
                    tracing::warn!(rev = %t.template_rev, error = %e, "template not readable");
                    rev_problem.insert(t.template_rev.clone(), None);
                }
            }
        }

        // Fonts, if the assignment carried any as assets.
        self.assigned.font_bytes = a
            .assets
            .iter()
            .filter_map(|h| self.store.cas.get(h).ok())
            .filter(|b| looks_like_a_font(b))
            .collect();

        // Resolve every asset a staged template names. Once, here, rather than
        // per arm.
        //
        // Sniffed from the first few bytes, not read whole and tried: an
        // assignment says a hash, never what kind of file it is, and a clip is
        // gigabytes. Reading one into memory to discover it is not a PNG would
        // undo the entire reason video is decoded a frame at a time.
        self.assigned.images.clear();
        self.assigned.videos.clear();
        let mut assets: Vec<(String, String)> = Vec::new();
        for doc in self.assigned.templates.values() {
            for asset in &doc.assets {
                assets.push((asset.key.clone(), asset.hash.clone()));
            }
        }
        for (key, hash) in assets {
            if self.assigned.images.contains_key(&key) {
                continue;
            }
            let Ok(path) = self.store.cas.path_of(&hash) else {
                continue;
            };
            if looks_like_an_image(&path) {
                let Ok(bytes) = self.store.cas.get(&hash) else {
                    continue;
                };
                match pmg_media::decode_image(&bytes) {
                    Ok(d) => {
                        self.assigned.images.insert(
                            key.clone(),
                            (d.width, d.height, std::sync::Arc::new(d.rgba)),
                        );
                    }
                    // Named, with the key, because "the background did not
                    // appear" is otherwise unattributable.
                    Err(e) => tracing::warn!(key = %key, hash = %hash, error = %e,
                                             "could not decode an image asset"),
                }
            } else if let Err(e) = self.register_clip(&key, &path) {
                tracing::warn!(key = %key, hash = %hash, error = %e,
                               "an asset is neither an image nor a playable clip");
            }
        }

        // Field-driven media: an Item's data may carry content addresses (a
        // per-Item clip or picture). `resolve_media_fields` turns those into
        // asset keys that ARE the hash, so they are loaded here keyed by hash —
        // sniffed and registered exactly like a template's own assets. The
        // bytes are already staged: the assignment's asset list carries them.
        for it in &a.items {
            for v in it.data.values() {
                let pmg_proto::DataValue::Asset(hash) = v else {
                    continue;
                };
                if self.assigned.images.contains_key(hash) {
                    continue;
                }
                let Ok(path) = self.store.cas.path_of(hash) else {
                    continue;
                };
                if looks_like_an_image(&path) {
                    let Ok(bytes) = self.store.cas.get(hash) else {
                        continue;
                    };
                    match pmg_media::decode_image(&bytes) {
                        Ok(d) => {
                            self.assigned.images.insert(
                                hash.clone(),
                                (d.width, d.height, std::sync::Arc::new(d.rgba)),
                            );
                        }
                        Err(e) => tracing::warn!(hash = %hash, error = %e,
                                                 "could not decode an Item's picture"),
                    }
                } else if let Err(e) = self.register_clip(hash, &path) {
                    tracing::warn!(hash = %hash, error = %e,
                                   "an Item's media is neither an image nor a playable clip");
                }
            }
        }

        self.assigned.items.clear();
        let mut items = Vec::new();
        for it in &a.items {
            self.assigned.items.insert(
                it.id.clone(),
                (it.label.clone(), it.template_rev.clone(), it.data.clone()),
            );

            items.push(readiness_of(
                it,
                |rev| self.assigned.templates.contains_key(rev),
                &rev_problem,
            ));
        }

        for i in items.iter().filter(|i| i.is_partial()) {
            // Worth a line each: this is the state where the take button works
            // and does less than the operator expects, so it should be findable
            // in the log afterwards.
            tracing::warn!(
                item = %i.item,
                blocked = ?i.destinations.iter().filter(|d| !d.ready)
                    .map(|d| format!("{}/{}", d.output, d.layer)).collect::<Vec<_>>(),
                "armable, but not everywhere"
            );
        }

        if let Some(tx) = tx {
            let _ = tx.send(ToLink::Ready(pmg_proto::Ready {
                assignment_gen: a.gen,
                items,
            }));
        }
    }

    fn load_template(&self, rev: &str) -> Result<SceneDoc> {
        let bytes = self
            .store
            .cas
            .get(rev)
            .with_context(|| format!("{rev} is not staged"))?;
        Ok(pmg_scene::cbor::from_canonical(&bytes)?)
    }

    /// Run the node: a render thread per output group, and this thread owning
    /// the engine and the link.
    ///
    /// # Why a thread per GROUP and not per output
    ///
    /// An output group is a set of outputs sharing a clock (ADR 0011), so one
    /// thread pumped by that clock drives all of them in step. Giving each
    /// output its own thread would mean two threads pacing themselves against
    /// the same hardware clock and drifting apart in software for no reason;
    /// giving every output ONE thread would mean an output on a different
    /// reference waiting for a clock it does not follow.
    ///
    /// The threading model is therefore decided by the plant's wiring, not by
    /// how many outputs there happen to be.
    pub fn run(&mut self, rx: Receiver<FromLink>, tx: Sender<ToLink>) -> Result<()> {
        let mut groups: BTreeMap<String, Vec<OutputSpec>> = BTreeMap::new();
        for o in &self.opts.outputs {
            groups.entry(o.group.clone()).or_default().push(o.clone());
        }
        tracing::info!(
            outputs = self.opts.outputs.len(),
            groups = groups.len(),
            "starting render threads"
        );

        let engine = self.engine.clone();
        // One stop flag per GROUP, not one for the whole run. Reconfiguring the
        // wall must not interrupt the key feed, and a group is exactly the set
        // of outputs that share a clock and a thread (ADR 0011, ADR 0024).
        let mut running: BTreeMap<String, RunningGroup> = BTreeMap::new();
        // Say what we are RUNNING, unasked, as soon as the link is up.
        //
        // The node's own file is the authority for its hardware (ADR 0024), and
        // until this the server only ever learned that authority by being told
        // to change it. So Configure's editor opened on a stub — id and backend
        // and nothing else — for a node driving a DeckLink with audio on a
        // sound device, and pressing Send would have written the stub back.
        let _ = tx.send(ToLink::OutputsApplied(pmg_proto::OutputsAck {
            gen: 0,
            applied: self.opts.outputs.iter().map(config_from_spec).collect(),
            ..Default::default()
        }));
        for (name, specs) in groups {
            running.insert(name.clone(), spawn_group(&name, &specs, engine.clone())?);
        }

        let mut last_state = Instant::now();
        // The node's offset from UTC, read once. A clock graphic shows the
        // node's local time (ADR 0022); a countdown is arithmetic on instants
        // and needs no offset at all.
        let tz_offset_ms = local_utc_offset_ms();
        loop {
            // Timers first, before the wait: a countdown must tick whether or
            // not the server has anything to say, which is the whole point of
            // the node computing it.
            //
            // The wait is then exactly as long as the soonest timer allows, so
            // a clock changes ON the second rather than up to a poll interval
            // after it — and with no timers on air it is the ordinary poll and
            // costs one comparison per layer.
            let now = crate::link::now_ms() as i64;
            {
                let mut e = self.engine.lock().unwrap();
                for l in e.run_timers(&self.assigned, now, tz_offset_ms) {
                    tracing::debug!(layer = %l, "timer tick");
                }
            }
            let wait = {
                let e = self.engine.lock().unwrap();
                match e.next_timer_due_ms() {
                    Some(due) => Duration::from_millis(
                        (due - crate::link::now_ms() as i64).clamp(1, 200) as u64,
                    ),
                    None => Duration::from_millis(200),
                }
            };
            match rx.recv_timeout(wait) {
                Ok(FromLink::Cue(c, received_at, reply)) => {
                    // Arming happens under the engine lock, which briefly stalls
                    // every render thread. An arm is 0.3–2 ms against a 16.68 ms
                    // budget and a take is a few times a minute, so it costs at
                    // most one late frame on a take — measured, not assumed, in
                    // the M3c notes. Moving the arm outside the lock is the
                    // obvious next step if that ever shows up on air.
                    // Is this cue already stale by the time we can act on it?
                    //
                    // This argument was hard-coded `false`. The rejection
                    // branch, its reason code and its tests all existed, and
                    // the runtime simply never asked — so "reject a late take,
                    // never fire it" was true of the engine and not of the
                    // node. It surfaced end to end: the server gave up waiting
                    // at 750 ms while the node was busy staging a clip, then
                    // applied the take anyway when it got round to it. The
                    // operator was told the take failed and the graphic went on
                    // air, which is the dangerous direction of that lie.
                    //
                    // Measured from when the frame came OFF THE SOCKET, so it
                    // catches the case that actually bit: this process too busy
                    // to answer. It does NOT include network transit, because
                    // the frame carries no issue timestamp and the server's
                    // clock is not ours to trust. Closing that needs an
                    // `issued_at` on the wire and is worth doing separately.
                    // Everything else already waiting, so a run of takes to one
                    // layer can be collapsed to the last one (ADR 0017).
                    //
                    // `try_recv` only: this NEVER waits to see whether a newer
                    // cue might arrive. Superseding requires that the newer
                    // intent is already in hand, which means this can only make
                    // the node faster than it was, never later. An operator
                    // holding the arrow keys through a chorus gets the slide
                    // they stopped on instead of the layer flashing through
                    // every one they passed.
                    let mut batch = vec![FromLink::Cue(c, received_at, reply)];
                    while let Ok(m) = rx.try_recv() {
                        batch.push(m);
                    }

                    // Which entries a LATER take in the same batch replaces.
                    // Computed up front, then the batch is walked in order —
                    // non-cue messages included — because reordering an
                    // assignment past a cue would apply a take against content
                    // it was not written for.
                    let mut dead = vec![false; batch.len()];
                    for i in 0..batch.len() {
                        let FromLink::Cue(a, _, _) = &batch[i] else {
                            continue;
                        };
                        let Some(keys) = a.op.take_layers() else {
                            continue;
                        };
                        dead[i] = batch[i + 1..].iter().any(|m| {
                            matches!(m, FromLink::Cue(b, _, _)
                                if b.op.take_layers().as_ref() == Some(&keys))
                        });
                    }

                    let mut acted = false;
                    for (i, m) in batch.into_iter().enumerate() {
                        match m {
                            FromLink::Cue(c, received_at, reply) if dead[i] => {
                                let ack = self
                                    .engine
                                    .lock()
                                    .unwrap()
                                    .supersede(&c, crate::link::now_ms());
                                tracing::info!(cue = %c.cue_id, result = ?ack.result,
                                               "superseded before it fired");
                                answer(&tx, reply, ack);
                                let _ = received_at;
                            }
                            FromLink::Cue(c, received_at, reply) => {
                                let received_late = is_late(c.deadline_ms, received_at.elapsed());
                                let ack = {
                                    let mut e = self.engine.lock().unwrap();
                                    e.apply(
                                        &c,
                                        &self.assigned,
                                        crate::link::now_ms(),
                                        received_late,
                                    )
                                };
                                tracing::info!(cue = %c.cue_id, result = ?ack.result,
                                               reason = ?ack.reason, "cue");
                                answer(&tx, reply, ack);
                                acted = true;
                            }
                            // Anything that is not a cue goes back through the
                            // ordinary path, in the position it arrived in.
                            other => self.handle(other, &tx),
                        }
                    }
                    // One state report for the whole batch. Sending one per cue
                    // would publish the intermediate slides an operator scrolled
                    // past as though each had been on air.
                    if acted {
                        let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
                    }
                }
                // Reconfiguring outputs is handled HERE rather than in
                // `handle`, because the render groups live on this stack —
                // applying one means stopping a thread and starting another
                // (ADR 0024).
                Ok(FromLink::SetOutputs {
                    gen,
                    outputs,
                    force,
                }) => {
                    let ack = self.apply_outputs(&mut running, gen, outputs, force);
                    let _ = tx.send(ToLink::OutputsApplied(ack));
                    let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
                }
                Ok(other) => self.handle(other, &tx),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if last_state.elapsed() >= Duration::from_millis(500) {
                last_state = Instant::now();
                let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
            }
        }

        for (_, g) in running {
            g.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let _ = g.handle.join();
        }
        Ok(())
    }

    /// Everything that is not a cue.
    ///
    /// Split out so a batch of cues can replay the non-cue messages that were
    /// interleaved with them IN ORDER (ADR 0017). Handling them out of order
    /// would let a take be applied against an assignment it was not written
    /// for, which is a far worse bug than the one superseding solves.
    /// Apply a new output configuration (ADR 0024).
    ///
    /// Three rules, in order:
    ///
    /// 1. **An output with something ON AIR is not reconfigured** unless the
    ///    request says `force`. Closing a device takes the graphic with it, and
    ///    an operator who means that should have said so.
    /// 2. **Persist before applying.** The same ordering an assignment uses: a
    ///    node that applied a configuration it had not saved would come up
    ///    wrong after a power cut, which is the case the file exists for.
    /// 3. **Report what was APPLIED.** A group whose device will not open is
    ///    named in `problems`, and the node keeps running whatever else it can
    ///    — a node that accepts a broken configuration and goes dark is the
    ///    failure this path exists to avoid.
    fn apply_outputs(
        &mut self,
        running: &mut BTreeMap<String, RunningGroup>,
        gen: u64,
        wanted: Vec<pmg_proto::OutputConfig>,
        force: bool,
    ) -> pmg_proto::OutputsAck {
        let mut ack = pmg_proto::OutputsAck {
            gen,
            ..Default::default()
        };

        // What is on air right now, by output id.
        let on_air: std::collections::BTreeSet<String> = {
            let e = self.engine.lock().unwrap();
            e.outputs
                .iter()
                .filter(|o| o.layers.iter().any(|l| l.occupant.is_some()))
                .map(|o| o.id.clone())
                .collect()
        };

        let plan = plan_outputs(&self.opts.outputs, &wanted, &on_air, force);
        ack.refused_on_air = plan.refused_on_air.clone();
        let next = plan.next;
        let changed_groups = plan.changed_groups;

        // Persist what will actually be RUNNING, not what was asked for.
        //
        // A refused output keeps its old configuration, and the file has to say
        // so: saving the request instead would leave a change nobody applied
        // waiting to surprise somebody at the next boot — which is the silent
        // behaviour this whole path exists to end. A refused change is dropped,
        // and the operator retries with `force` if they mean it.
        let persist: Vec<pmg_proto::OutputConfig> = next.iter().map(config_from_spec).collect();
        if let Err(e) = self.store.save_outputs(&persist) {
            ack.problems
                .push(format!("could not save the configuration: {e}"));
            return ack;
        }
        self.opts.outputs = next.clone();

        // Rebuild only the groups that changed.
        let mut by_group: BTreeMap<String, Vec<OutputSpec>> = BTreeMap::new();
        for o in &next {
            by_group.entry(o.group.clone()).or_default().push(o.clone());
        }
        for g in &changed_groups {
            if let Some(old) = running.remove(g) {
                old.stop.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = old.handle.join();
            }
            let Some(specs) = by_group.get(g) else {
                continue; // the group is empty now, and stopping it was the job
            };
            match spawn_group(g, specs, self.engine.clone()) {
                Ok(rg) => {
                    running.insert(g.clone(), rg);
                }
                Err(e) => ack
                    .problems
                    .push(format!("group {g} could not be started: {e}")),
            }
        }

        // The layer engine is told about the outputs it now has. Layers and
        // colour still come from the assignment — this only adds or removes the
        // outputs themselves (ADR 0024).
        {
            let mut e = self.engine.lock().unwrap();
            let fps = next.first().map(|o| o.fps).unwrap_or((60000, 1001));
            e.redeclare_outputs(
                &next
                    .iter()
                    .map(|o| (o.id.clone(), o.layers.clone()))
                    .collect::<Vec<_>>(),
                fps,
            );
        }

        // What it APPLIED. The contract this type states, and for a moment it
        // echoed the request instead — which would have shown an operator their
        // refused rename as though it had taken.
        ack.applied = persist;
        ack
    }

    fn handle(&mut self, m: FromLink, tx: &Sender<ToLink>) {
        match m {
            // Handled in `run`, where the render groups live.
            FromLink::SetOutputs { .. } => {}
            FromLink::Connected { epoch } => {
                self.engine.lock().unwrap().set_epoch(epoch);
                let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
            }
            FromLink::Tick => {
                let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
            }
            FromLink::Disconnected => {
                // Deliberately nothing. What is on air stays on air.
                tracing::warn!("link down — still playing");
            }
            FromLink::Assignment(a) => {
                // Persist FIRST, then adopt, then ack. Acking before the fsync
                // would claim durability we do not have (ADR 0009).
                match self.store.save_assignment(&a) {
                    Ok(()) => {
                        self.adopt(&a, Some(tx));
                        let _ = tx.send(ToLink::AssignmentSaved { gen: a.gen });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "could not persist assignment; NOT acking")
                    }
                }
            }
            FromLink::Resync(reason) => {
                tracing::warn!(%reason, "server asked for a resync");
            }
            // Handled by the batching path in `run`, which is the only place
            // that can see a later cue superseding this one.
            FromLink::Cue(..) => unreachable!("cues are handled in the batch loop"),
        }
    }

    pub fn engine(&self) -> &Arc<Mutex<LayerEngine>> {
        &self.engine
    }
}

impl Store {
    fn has_str(&self, hash: &str) -> bool {
        self.cas.has(hash)
    }
}

impl Runtime {
    /// Probe a clip and decode its first frame.
    ///
    /// The first frame is what an armed clip shows before its decoder has
    /// produced anything, and it is also what sizes the texture the player will
    /// overwrite — so a clip that cannot be probed fails **here**, at assignment,
    /// rather than as a blank rectangle at 10:15 on Sunday.
    ///
    /// Costs about 80 ms per clip, once, on the assignment path. That is the
    /// same trade as decoding every still here rather than at each arm.
    fn register_clip(&mut self, key: &str, path: &std::path::Path) -> Result<()> {
        let spec = pmg_media::video::probe(path)?;
        let dec = pmg_media::video::Decoder::open(path, &spec, 0, false)?;
        let mut player =
            pmg_media::video::VideoPlayer::new(dec, (spec.fps_num, spec.fps_den), false);
        if !player.preroll(Duration::from_secs(10)) {
            anyhow::bail!("no frame decoded within 10 s");
        }
        let poster = player
            .frame_for(0)
            .map(|f| f.rgba.clone())
            .context("no first frame")?;

        tracing::info!(
            key = %key, codec = %spec.codec, w = spec.width, h = spec.height,
            fps = %format!("{}/{}", spec.fps_num, spec.fps_den),
            alpha = spec.has_alpha, frames = spec.frames,
            "staged a clip"
        );
        self.assigned
            .images
            .insert(key.to_string(), (spec.width, spec.height, poster));
        self.assigned
            .videos
            .insert(key.to_string(), (path.to_path_buf(), spec));
        Ok(())
    }
}

/// Is this file a still, by its magic bytes?
///
/// Reads a header, never the file. The alternative — read it all and see if it
/// decodes — costs a gigabyte of RAM to learn that a clip is not a PNG.
fn looks_like_an_image(path: &std::path::Path) -> bool {
    use std::io::Read;
    let mut head = [0u8; 4];
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    if f.read_exact(&mut head).is_err() {
        return false;
    }
    head.starts_with(&[0x89, b'P', b'N', b'G']) || head.starts_with(&[0xFF, 0xD8, 0xFF])
}

/// A cheap sniff for the font formats swash reads.
///
/// Assignments carry assets without saying what they are, and handing a PNG to
/// the font database produces an error at arm time — long after the useful
/// moment to notice.
fn looks_like_a_font(b: &[u8]) -> bool {
    matches!(
        b.get(0..4),
        Some(b"\x00\x01\x00\x00") | Some(b"true") | Some(b"OTTO") | Some(b"ttcf") | Some(b"wOFF")
    )
}

/// When to sample the settled audio buffer and adopt it as the sync target.
///
/// Ten seconds in: past preroll and past the first clip's decoder warming up,
/// while still early enough that a target learned from a disturbed level is
/// unlikely.
const LEARN_TARGET_FRAME: u64 = 600;

/// Has a cue's deadline passed before we could act on it?
///
/// A tiny function so it can be TESTED. The engine's rejection branch had a
/// test from the start; the runtime that decides when to take it did not, and
/// passed a hard-coded `false` for months. The rule was true of the engine and
/// false of the node, which is the failure mode a unit test on the callee
/// cannot see.
///
/// `None` means the op carries no deadline — an arm or a preload, which are not
/// time-critical and must not be refused for being slow.
/// This machine's offset from UTC, in milliseconds.
///
/// Read from the C library rather than pulled in as a dependency: a booth Mac
/// has no internet and the tz database it already carries is the one its own
/// clock uses. A failure reads as UTC, which is wrong by hours rather than
/// wrong by a little — the sort of wrong somebody notices immediately, which
/// is the right way for a clock to fail.
fn local_utc_offset_ms() -> i64 {
    // `date +%z` is +HHMM. Shelling out once at startup is cheaper in
    // dependencies than a tz crate, and this is the only place that needs it.
    let out = std::process::Command::new("date").arg("+%z").output().ok();
    let text = out
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_default();
    let t = text.trim();
    let parse = || -> Option<i64> {
        let sign = match t.as_bytes().first()? {
            b'-' => -1,
            b'+' => 1,
            _ => return None,
        };
        let h: i64 = t.get(1..3)?.parse().ok()?;
        let m: i64 = t.get(3..5)?.parse().ok()?;
        Some(sign * (h * 3_600_000 + m * 60_000))
    };
    parse().unwrap_or(0)
}

fn is_late(deadline_ms: Option<u64>, elapsed: std::time::Duration) -> bool {
    deadline_ms.is_some_and(|d| elapsed.as_millis() as u64 > d)
}

/// What one layer needs drawn this frame, taken under the lock and used without
/// it.
struct LayerPlan {
    layer: String,
    inst: Arc<pmg_render::scene::SceneInstance>,
    values: std::collections::BTreeMap<String, f32>,
    players: Option<Arc<std::sync::Mutex<engine::PlayerSet>>>,
    /// A live rebind morphing to this instance: what was on screen, and how far
    /// through. `None` in the steady state, which is almost always.
    tween: Option<(Arc<pmg_render::scene::SceneInstance>, f32)>,
}

/// Pull this frame's picture for every clip on a layer and upload it.
///
/// The three rules from `docs/RISKS.md` R5, in one function:
///
/// - **Never stall the output.** Every call here is non-blocking. A clip whose
///   decoder has not kept up simply does not write, so the texture keeps the
///   last picture — which is the definition of holding a frame.
/// - **The output is the clock.** The media frame is derived from this output's
///   own frame counter, not from wall time, so a clip stays locked to the signal
///   it is going out on rather than to how the machine felt.
/// - **The clock starts when the first frame lands**, not at the take. See
///   [`engine::PlayerSet::start_frame`].
fn advance_clips(
    gpu: &crate::render::Gpu,
    renderer: &pmg_render::SceneRenderer,
    set: &Arc<std::sync::Mutex<engine::PlayerSet>>,
    out_frame: u64,
    audio_samples: usize,
    audio_out: &mut [i32],
) {
    // A poisoned lock here means a render thread panicked mid-frame. Skipping
    // the clip beats propagating the panic and taking the whole output down.
    let Ok(mut set) = set.lock() else { return };

    if set.start_frame.is_none() {
        // Zero timeout: this asks "has the first frame arrived", it does not
        // wait for one. The waiting version of this call belongs at arm.
        let ready = set
            .players
            .iter_mut()
            .all(|(_, p)| p.preroll(std::time::Duration::ZERO));
        if !ready {
            return;
        }
        set.start_frame = Some(out_frame);
    }
    let n = out_frame.saturating_sub(set.start_frame.unwrap_or(out_frame));

    for (slot, player) in &mut set.players {
        let slot = *slot;
        let (w, h) = (player.spec().width, player.spec().height);
        if let Some(frame) = player.frame_for(n) {
            // A refused write means the clip's size stopped matching the slot,
            // which cannot happen without a bug upstream. It is dropped rather
            // than stretched: a partially written frame looks like tearing and
            // would hide the real fault.
            let _ = renderer.update_image(&gpu.queue, slot, w, h, &frame.rgba);
        }
    }

    // Audio for the SAME frame number, taken with the count the output's clock
    // asked for. Summed across this layer's clips, which is almost always one —
    // and mixed into whatever earlier layers already put there, so two layers
    // with sound do not silence each other.
    if audio_samples > 0 && !set.audio.is_empty() {
        for dec in &mut set.audio {
            let pcm = dec.take(audio_samples);
            for (dst, src) in audio_out.iter_mut().zip(pcm.iter()) {
                *dst = dst.saturating_add(*src);
            }
        }
    }
}

/// One output group: its own GPU device, its own backends, its own clock.
///
/// Locks the engine only long enough to take a **snapshot** of what each of its
/// outputs should be showing — an `Arc<SceneInstance>` plus this frame's
/// timeline values — and then draws with the lock released. Holding it across a
/// draw would serialise every group into one thread wearing several hats.
/// One wire-level `OutputConfig` as the runtime's own `OutputSpec`.
///
/// LAYERS are carried across from whatever the output already had, because they
/// belong to the assignment and not to this message — hardware and content are
/// configured separately (ADR 0024). A brand-new output starts with the
/// conventional single stack so it is usable before a server says anything.
pub fn spec_from_config(
    cfg: &pmg_proto::OutputConfig,
    existing: Option<&OutputSpec>,
) -> OutputSpec {
    OutputSpec {
        id: cfg.id.clone(),
        layers: existing
            .map(|e| e.layers.clone())
            .unwrap_or_else(|| vec!["Graphics".to_string()]),
        fps: (cfg.fps_num.max(1), cfg.fps_den.max(1)),
        width: cfg.width.max(1),
        height: cfg.height.max(1),
        backend: cfg.backend.clone(),
        ndi_name: if cfg.ndi_name.is_empty() {
            format!("ProdMesh Graphics {}", cfg.id)
        } else {
            cfg.ndi_name.clone()
        },
        decklink_device: cfg.device as usize,
        decklink_key_device: cfg.key_device.map(|d| d as usize),
        decklink_mode: cfg.mode.clone(),
        preview_port: cfg.preview_port,
        group: if cfg.group.is_empty() {
            "default".to_string()
        } else {
            cfg.group.clone()
        },
        audio_channels: cfg.audio_channels,
        audio_device: cfg.audio_device.clone(),
        audio_host: cfg.audio_host.clone(),
        // A test instrument, never configuration.
        inject_stall_at_s: existing.and_then(|e| e.inject_stall_at_s),
        preview_mjpeg: existing.is_some_and(|e| e.preview_mjpeg),
    }
}

/// Does this output still open the same hardware the same way?
///
/// Only these fields mean closing a device and opening it again. Layers and
/// colour change without interrupting a frame, which is why they travel in the
/// assignment and are not compared here.
///
/// **Compared per backend**, because a field one backend has never heard of
/// must not interrupt it. An NDI sender was being torn down and rebuilt because
/// a DeckLink display-mode string differed — a frame of black on the wall, for
/// a value that output does not read.
fn same_hardware(a: &OutputSpec, b: &OutputSpec) -> bool {
    let common = a.backend == b.backend
        && a.preview_port == b.preview_port
        && a.group == b.group
        && a.audio_channels == b.audio_channels
        // The sound device is HARDWARE: changing it means closing one device
        // and opening another, which is a group restart exactly as a DeckLink
        // device change is (ADR 0024).
        && a.audio_device == b.audio_device
        && a.audio_host == b.audio_host
        && a.fps == b.fps
        && a.width == b.width
        && a.height == b.height;
    if !common {
        return false;
    }
    match a.backend.as_str() {
        "ndi" => a.ndi_name == b.ndi_name,
        "decklink" => {
            a.decklink_device == b.decklink_device
                && a.decklink_key_device == b.decklink_key_device
                && a.decklink_mode == b.decklink_mode
        }
        // `window` and `headless` have nothing else to open.
        _ => true,
    }
}

/// An `OutputSpec` as the wire describes it — the inverse of
/// [`spec_from_config`], used to seed a node's file from the flags it was
/// started with.
pub fn config_from_spec(o: &OutputSpec) -> pmg_proto::OutputConfig {
    pmg_proto::OutputConfig {
        id: o.id.clone(),
        backend: o.backend.clone(),
        ndi_name: o.ndi_name.clone(),
        device: o.decklink_device as u32,
        key_device: o.decklink_key_device.map(|d| d as u32),
        mode: o.decklink_mode.clone(),
        width: o.width,
        height: o.height,
        fps_num: o.fps.0,
        fps_den: o.fps.1,
        audio_channels: o.audio_channels,
        audio_device: o.audio_device.clone(),
        audio_host: o.audio_host.clone(),
        preview_port: o.preview_port,
        group: o.group.clone(),
    }
}

/// What a reconfiguration would do, decided before anything is torn down.
pub struct OutputPlan {
    /// The specs the node should be running afterwards.
    pub next: Vec<OutputSpec>,
    /// The render groups that have to be stopped and started again.
    pub changed_groups: std::collections::BTreeSet<String>,
    /// Outputs left alone because something is on air on them.
    pub refused_on_air: Vec<String>,
}

/// Work out what changes, and what that interrupts (ADR 0024).
///
/// Pure, and separate from applying it, because these are the rules — an
/// output with a graphic on it is not reconfigured without `force`; only the
/// groups that actually change are interrupted; an output that leaves takes its
/// old group down with it — and rules are worth testing without spawning a
/// render thread to do it.
pub fn plan_outputs(
    current: &[OutputSpec],
    wanted: &[pmg_proto::OutputConfig],
    on_air: &std::collections::BTreeSet<String>,
    force: bool,
) -> OutputPlan {
    let by_id: BTreeMap<&str, &OutputSpec> = current.iter().map(|o| (o.id.as_str(), o)).collect();
    let mut plan = OutputPlan {
        next: Vec::new(),
        changed_groups: std::collections::BTreeSet::new(),
        refused_on_air: Vec::new(),
    };

    for cfg in wanted {
        let existing = by_id.get(cfg.id.as_str()).copied();
        let spec = spec_from_config(cfg, existing);
        if existing.is_some_and(|e| same_hardware(e, &spec)) {
            // Nothing that means closing a device. Layers and colour still
            // arrive by assignment, without interrupting a frame.
            plan.next.push(spec);
            continue;
        }
        if on_air.contains(&cfg.id) && !force {
            plan.refused_on_air.push(cfg.id.clone());
            // Keep what is running, precisely because something is on it.
            if let Some(e) = existing {
                plan.next.push(e.clone());
            }
            continue;
        }
        plan.changed_groups.insert(spec.group.clone());
        if let Some(e) = existing {
            // The group it is LEAVING is rebuilt too, or the old thread keeps
            // driving a device the new one wants.
            plan.changed_groups.insert(e.group.clone());
        }
        plan.next.push(spec);
    }

    // An output that is gone takes its group down with it.
    for o in current {
        if wanted.iter().any(|c| c.id == o.id) {
            continue;
        }
        if on_air.contains(&o.id) && !force {
            plan.refused_on_air.push(o.id.clone());
            plan.next.push(o.clone());
            continue;
        }
        plan.changed_groups.insert(o.group.clone());
    }

    plan
}

/// A render group that is currently running: its stop flag and its thread.
///
/// Kept per group so one can be torn down and rebuilt while the others keep
/// their frames. Reconfiguring an output means closing a device and opening it
/// again, and that interruption should reach exactly the outputs it has to.
struct RunningGroup {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

fn spawn_group(
    name: &str,
    specs: &[OutputSpec],
    engine: Arc<Mutex<LayerEngine>>,
) -> Result<RunningGroup> {
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let group_name = name.to_string();
    let specs = specs.to_vec();
    let s = stop.clone();
    let handle = std::thread::Builder::new()
        .name(format!("pmg-out-{name}"))
        .spawn(move || {
            if let Err(e) = render_group(&group_name, &specs, engine, s) {
                // A dead output must be loud. It is also not a reason to take
                // the others down.
                tracing::error!(group = %group_name, error = %e, "render group stopped");
            }
        })?;
    Ok(RunningGroup { stop, handle })
}

fn render_group(
    group: &str,
    specs: &[OutputSpec],
    engine: Arc<Mutex<LayerEngine>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Result<()> {
    let gpu = Gpu::open()?;
    tracing::info!(group, adapter = %gpu.adapter_name, outputs = specs.len(), "render group up");

    struct Live {
        spec: OutputSpec,
        target: Target,
        backend: Backend,
        renderers: crate::render::RendererCache,
        /// Atlases for instances being morphed AWAY from. Separate cache because
        /// they are keyed the same as the live ones and must not evict them.
        tween_renderers: crate::render::RendererCache,
        preview: Option<pmg_preview::Preview>,
        /// H.264 for `VideoDecoder`, which is what the browser actually uses.
        /// MJPEG stays available because it needs no decoder at all and is
        /// therefore the instrument that answers "is ANYTHING coming out".
        preview_h264: Option<pmg_preview::h264::H264Preview>,
        /// Preview size and the box-filter step, so the downscale is done once
        /// and handed to whichever transports are up.
        preview_dims: (u32, u32, u32),
        frame: u64,
        /// This frame's interleaved audio. Reused across frames so a service
        /// does not allocate a buffer per frame.
        audio: Vec<i32>,
        /// Where this output's audio goes when it does NOT ride the video
        /// (ADR 0028). `None` is embedded, which is what every output was.
        audio_sink: Option<pmg_out_audio::Sink>,
        /// `None` when this output carries no audio.
        aclock: Option<pmg_audio::AudioClock>,
        /// Closes the loop on the card's audio buffer. `None` until the level
        /// has settled — see `learn_target`.
        async_: Option<pmg_audio::AudioSync>,
    }

    let mut outputs = Vec::new();
    for spec in specs {
        let target = Target::new(&gpu, spec.width, spec.height)?;
        let backend = open_backend(spec)?;
        // 480 across, whatever the output is. A confidence monitor, not a
        // scaled copy of the programme feed.
        let pstep = (spec.width / 480).max(1);
        let preview_dims = (spec.width / pstep, spec.height / pstep, pstep);

        let preview_h264 = if spec.preview_port == 0 || spec.preview_mjpeg {
            None
        } else {
            match pmg_preview::h264::H264Preview::start(pmg_preview::h264::Config {
                width: preview_dims.0,
                height: preview_dims.1,
                fps_num: spec.fps.0,
                fps_den: spec.fps.1,
                port: spec.preview_port,
                bitrate: 4_000_000,
            }) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!(output = %spec.id, error = %e,
                                   "H.264 preview unavailable; falling back to MJPEG");
                    None
                }
            }
        };

        let preview = if spec.preview_port == 0 || preview_h264.is_some() {
            None
        } else {
            match pmg_preview::Preview::start(pmg_preview::Config {
                source_width: spec.width,
                source_height: spec.height,
                port: spec.preview_port,
                ..Default::default()
            }) {
                Ok(p) => Some(p),
                Err(e) => {
                    // A preview that cannot start is not a reason to refuse to
                    // play. It is a monitoring feature.
                    tracing::warn!(output = %spec.id, error = %e, "preview unavailable");
                    None
                }
            }
        };
        outputs.push(Live {
            spec: spec.clone(),
            target,
            backend,
            renderers: crate::render::RendererCache::default(),
            tween_renderers: crate::render::RendererCache::default(),
            audio: Vec::new(),
            // Opened here, with the backend, because it is hardware and shares
            // the group's lifetime: reconfiguring the device restarts the group
            // exactly as changing a DeckLink port does (ADR 0024).
            audio_sink: if spec.audio_device.is_empty() || spec.audio_channels <= 0 {
                None
            } else {
                let target = (pmg_audio::SAMPLE_RATE as u64 * spec.fps.1 as u64 * 3
                    / spec.fps.0.max(1) as u64) as u32;
                match pmg_out_audio::Sink::open(&pmg_out_audio::Config {
                    host: Some(spec.audio_host.clone()).filter(|h| !h.is_empty()),
                    device: Some(spec.audio_device.clone()),
                    channels: spec.audio_channels.max(0) as u16,
                    target_frames: target,
                }) {
                    Ok(sink) => Some(sink),
                    Err(e) => {
                        // Loud, and NOT fatal. A sound device that will not open
                        // must not take the picture off air with it — the show
                        // continues silent, and the node says why on its status
                        // page rather than dying in a log nobody is reading.
                        tracing::error!(
                            output = %spec.id, device = %spec.audio_device,
                            "audio device would not open, this output runs SILENT: {e:#}",
                        );
                        None
                    }
                }
            },
            async_: None,
            aclock: match spec.audio_channels {
                0 => None,
                _ => match pmg_audio::AudioClock::new(spec.fps, pmg_audio::SAMPLE_RATE) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        // Refused loudly rather than played at the wrong rate:
                        // audio at 44.1 on a 48 kHz plant is a pitch shift.
                        tracing::error!(output = %spec.id, error = %e, "audio disabled");
                        None
                    }
                },
            },
            preview,
            preview_h264,
            preview_dims,
            frame: 0,
        });
    }

    let mut sync_poll = Instant::now();
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        for out in &mut outputs {
            let budget_ms = 1000.0 / (out.spec.fps.0 as f64 / out.spec.fps.1 as f64);
            let t0 = Instant::now();

            // The injected stall. One second, once, at the requested time —
            // long enough to leave the audio offset the soak found.
            if let Some(at) = out.spec.inject_stall_at_s {
                let at_frame = at * out.spec.fps.0 as u64 / out.spec.fps.1 as u64;
                if out.frame == at_frame {
                    tracing::warn!(output = %out.spec.id, frame = out.frame,
                                   "INJECTING a 1 s stall (test instrument)");
                    std::thread::sleep(Duration::from_millis(1000));
                }
            }

            // --- snapshot, under the lock, briefly ------------------------
            let (plan, pattern) = {
                let e = engine.lock().unwrap();
                let Some(o) = e.outputs.iter().find(|o| o.id == out.spec.id) else {
                    continue;
                };
                // Outgoing FIRST, so a dissolving graphic is drawn under the
                // one arriving. Painter's order within a layer: the new slide
                // comes up over the old rather than behind it.
                let plan: Vec<LayerPlan> = o
                    .layers
                    .iter()
                    .flat_map(|l| {
                        let one = |occ: &engine::Occupant| LayerPlan {
                            layer: l.name.clone(),
                            inst: occ.armed.instance.clone(),
                            values: engine::frame_values(occ),
                            // An Arc clone, so the players come out of the lock
                            // with everything else and are touched outside it.
                            players: occ.armed.players.clone(),
                            tween: occ.tween.as_ref().map(|t| (t.from.clone(), t.t())),
                        };
                        l.outgoing
                            .iter()
                            .chain(l.occupant.iter())
                            .map(one)
                            .collect::<Vec<_>>()
                    })
                    .collect();
                (plan, o.test_pattern.clone())
            };

            // --- draw, with the lock released -----------------------------
            // This frame's audio buffer, sized by the OUTPUT's clock. Cleared
            // and refilled rather than reallocated — a service must not
            // allocate a buffer a frame.
            // The exact count for this frame, then the correction on top.
            //
            // AudioClock alone is open loop: it counts frames we SCHEDULED,
            // while the card consumes against its own 48 kHz clock. A stall
            // leaves audio permanently ahead — measured at +168 ms over a
            // two-hour soak, with nothing correcting it (INTEGRATION-NOTES).
            let nominal = out.aclock.map(|c| c.samples_for(out.frame)).unwrap_or(0);
            let asamples = if nominal == 0 {
                0
            } else {
                // Whichever device actually holds this audio is the one to
                // correct against. With a separate sound device that is the
                // sink's ring — the video output still sets the RATE, the sink
                // reports the ERROR, and the two clocks are held together by
                // exactly the same loop (ADR 0028).
                let buffered = match (&out.audio_sink, &out.backend) {
                    (Some(sink), _) => {
                        // Not yet draining: the ring is still filling to its
                        // working depth and its level says nothing about drift.
                        if sink.primed() {
                            sink.buffered()
                        } else {
                            0
                        }
                    }
                    (None, Backend::DeckLink(d)) => d.stats().buffered_audio as u32,
                    // NDI paces itself and exposes no buffer to read, so there
                    // is nothing to close the loop against. Stated rather than
                    // silently skipped.
                    _ => 0,
                };
                match (&mut out.async_, buffered) {
                    (Some(sync), b) if b > 0 => sync.correct(nominal, b) as usize,
                    // Learn the natural level once it has settled rather than
                    // hard-coding one: it depends on preroll depth and on the
                    // card, and a wrong target would be corrected TOWARDS
                    // forever.
                    (None, b) if b > 0 && out.frame == LEARN_TARGET_FRAME => {
                        tracing::info!(output = %out.spec.id, target = b,
                                       "audio sync target learned");
                        out.async_ = Some(pmg_audio::AudioSync::new(b));
                        nominal as usize
                    }
                    _ => nominal as usize,
                }
            };
            out.audio.clear();
            out.audio
                .resize(asamples * out.spec.audio_channels.max(0) as usize, 0);

            let mut live = std::collections::HashSet::new();
            for (i, p) in plan.iter().enumerate() {
                live.insert(p.layer.clone());
                let renderer = out.renderers.get(&gpu, &p.layer, &p.inst);
                // Clips first: a frame that arrives after the draw is a frame
                // late, and the texture write must be queued before the pass
                // that samples it.
                if let Some(set) = &p.players {
                    advance_clips(&gpu, renderer, set, out.frame, asamples, &mut out.audio);
                }
                let load = if i == 0 {
                    pmg_render::Load::Clear
                } else {
                    pmg_render::Load::Over
                };

                match &p.tween {
                    // A rebind in flight: the new layout carrying the geometry,
                    // then the outgoing content dissolved over it. Two passes,
                    // and only while a tween is actually running.
                    Some((from, t)) => {
                        let from_renderer = out.tween_renderers.get(&gpu, &p.layer, from);
                        let frames = pmg_render::tween_frames(from, &p.inst, *t, &p.values);
                        crate::render::draw_frames(
                            &gpu,
                            renderer,
                            &p.inst,
                            &frames.to,
                            &out.target,
                            load,
                        );
                        crate::render::draw_frames(
                            &gpu,
                            from_renderer,
                            from,
                            &frames.from,
                            &out.target,
                            pmg_render::Load::Over,
                        );
                    }
                    None => {
                        crate::render::draw(&gpu, renderer, &p.inst, &p.values, &out.target, load);
                    }
                }
            }
            if plan.is_empty() {
                out.target.clear(&gpu);
            }
            out.renderers.retain(&live);
            // Outgoing atlases live only as long as their tween. Retaining on
            // the same set as the live ones would hold a second atlas per layer
            // for the rest of the service.
            let morphing: std::collections::HashSet<String> = plan
                .iter()
                .filter(|p| p.tween.is_some())
                .map(|p| p.layer.clone())
                .collect();
            out.tween_renderers.retain(&morphing);

            let mut rgba = out.target.read(&gpu)?;
            if let Some(name) = &pattern {
                if let Some(p) = pmg_color::patterns::Pattern::parse(name) {
                    let f =
                        pmg_color::patterns::render(p, out.spec.width, out.spec.height, out.frame);
                    // The generator works in STRAIGHT alpha, which is the only
                    // way the alpha staircase means anything; the boundary
                    // wants premultiplied like every other frame.
                    rgba = pmg_color::patterns::to_rgba8_premultiplied(&f);
                }
            }
            out.frame += 1;

            // Audio has no error to check — the API takes a wrong sample count
            // without complaint and the fault is a slow walk out of lip sync.
            // The card's own buffer level is the only instrument, so a node
            // carrying audio says it out loud every five seconds: FLAT is
            // correct, a trend in either direction is not.
            // Clip health, from the players themselves. `late` says the OUTPUT
            // kept up; `held` says the DECODER did, and they fail separately:
            // a held frame is a repeated picture on a perfectly timed output.
            if out.frame.is_multiple_of(300) {
                let (mut held, mut dropped) = (0u64, 0u64);
                for p in &plan {
                    if let Some(set) = &p.players {
                        if let Ok(s) = set.lock() {
                            for (_, pl) in &s.players {
                                held += pl.held();
                                dropped += pl.dropped();
                            }
                        }
                    }
                }
                if !plan.is_empty() {
                    tracing::info!(
                        output = %out.spec.id, frame = out.frame,
                        held, dropped, "clips"
                    );
                }
            }

            if out.spec.audio_channels > 0 && out.frame.is_multiple_of(300) {
                // NDI has no card buffer to read — the SDK owns the pacing —
                // so there is one fewer instrument here, and the peak is the
                // one that still means something.
                if let Backend::Ndi(_) = &out.backend {
                    let peak = out
                        .audio
                        .iter()
                        .map(|s| s.unsigned_abs())
                        .max()
                        .unwrap_or(0);
                    let dbfs = if peak == 0 {
                        f32::NEG_INFINITY
                    } else {
                        20.0 * (peak as f32 / i32::MAX as f32).log10()
                    };
                    tracing::info!(
                        output = %out.spec.id,
                        frame = out.frame,
                        peak_dbfs = format!("{dbfs:.1}"),
                        "audio"
                    );
                }
                if let Backend::DeckLink(d) = &out.backend {
                    let st = d.stats();
                    // The peak of THIS frame's samples, as dBFS. A flat buffer
                    // level proves the cadence is right and says nothing at all
                    // about what is in the samples — feeding exactly the right
                    // number of zeroes looks identical. This is the difference
                    // between "audio is timed correctly" and "audio is there".
                    let peak = out
                        .audio
                        .iter()
                        .map(|s| s.unsigned_abs())
                        .max()
                        .unwrap_or(0);
                    let dbfs = if peak == 0 {
                        f32::NEG_INFINITY
                    } else {
                        20.0 * (peak as f32 / i32::MAX as f32).log10()
                    };
                    tracing::info!(
                        output = %out.spec.id,
                        frame = out.frame,
                        audio_buffered = st.buffered_audio,
                        audio_ms = st.buffered_audio * 1000 / pmg_audio::SAMPLE_RATE as u64,
                        peak_dbfs = format!("{dbfs:.1}"),
                        late = st.displayed_late,
                        target = out.async_.map(|s| s.target()).unwrap_or(0),
                        corrections = out.async_.map(|s| s.corrections).unwrap_or(0),
                        resyncs = out.async_.map(|s| s.resyncs).unwrap_or(0),
                        "audio buffer"
                    );
                }
            }

            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if ms > budget_ms {
                if let Ok(mut e) = engine.lock() {
                    if let Some(o) = e.outputs.iter_mut().find(|o| o.id == out.spec.id) {
                        o.late_frames += 1;
                    }
                }
            }

            // ONE destination for the audio, never two. The sample count per
            // frame is corrected against exactly one buffer, so a second
            // destination would have nothing holding it in step and would walk
            // away over a service (ADR 0028).
            let embedded: &[i32] = if out.audio_sink.is_some() {
                &[]
            } else {
                &out.audio
            };
            match &out.backend {
                // NDI is opened with clock_video, so this call paces the loop.
                Backend::Ndi(n) => {
                    n.send_rgba_premultiplied(&rgba)?;
                    // Audio AFTER the picture, so a video failure never leaves
                    // a receiver holding sound for a frame that will not come.
                    n.send_audio_interleaved_32s(
                        embedded,
                        if embedded.is_empty() {
                            0
                        } else {
                            out.spec.audio_channels.max(0) as usize
                        },
                    )?;
                }
                // DeckLink schedules against its own clock and blocks when the
                // queue is full — the same pacing, against a clock that can be
                // genlocked.
                Backend::DeckLink(d) => d.send_rgba_premultiplied_with_audio(&rgba, embedded)?,
                Backend::Headless => std::thread::sleep(Duration::from_millis(budget_ms as u64)),
            }
            // The sound device is fed AFTER the picture, for the same reason
            // NDI's audio is: a video failure must not leave a plant holding
            // sound for a frame that will never come.
            if let Some(sink) = &out.audio_sink {
                sink.push(&out.audio);
            }

            // Downscale ONCE, then hand the same pixels to whichever transport
            // is up. Doing it per transport would pay for the checkerboard
            // composite twice for one picture.
            if out.preview.is_some() || out.preview_h264.is_some() {
                let (pw, ph, step) = out.preview_dims;
                let small = pmg_preview::downscale_over_checkerboard(&rgba, pw, ph, step);
                if let Some(p) = &out.preview_h264 {
                    p.offer(small);
                } else if let Some(p) = &out.preview {
                    p.offer_rgb(small);
                }
            }

            // Audio levels for the same viewer, ~10 Hz: this frame's peak per
            // CHANNEL, from the samples that just went to the wire — so the
            // meter reads what the plant hears, not what a decoder produced.
            // Cheap enough not to gate: one pass over one frame's samples,
            // every sixth frame, only while something previews.
            if out.spec.audio_channels > 0 && out.frame.is_multiple_of(6) {
                if let Some(p) = &out.preview_h264 {
                    let ch = out.spec.audio_channels as usize;
                    let mut peaks = vec![0u32; ch];
                    for (i, s) in out.audio.iter().enumerate() {
                        let c = i % ch;
                        peaks[c] = peaks[c].max(s.unsigned_abs());
                    }
                    // −120 dBFS stands in for silence: JSON has no infinity,
                    // and no meter distinguishes below that anyway.
                    let dbfs: Vec<f32> = peaks
                        .iter()
                        .map(|&pk| {
                            if pk == 0 {
                                -120.0
                            } else {
                                (20.0 * (pk as f32 / i32::MAX as f32).log10()).max(-120.0)
                            }
                        })
                        .collect();
                    if let Ok(json) = serde_json::to_vec(&serde_json::json!({ "levels": dbfs })) {
                        p.offer_levels(json);
                    }
                }
            }
        }

        // Tick every output in this group together, which is what sharing a
        // clock means: their timelines advance on the same beat.
        if let Ok(mut e) = engine.lock() {
            for out in &outputs {
                e.tick(&out.spec.id);
            }
        }

        // Refresh reference status about once a second. A reference can be
        // unplugged mid-service, so this is polled rather than read at open —
        // a status that only reflects boot time is worse than none, because it
        // is confidently stale.
        if sync_poll.elapsed() >= Duration::from_secs(1) {
            sync_poll = Instant::now();
            for out in &outputs {
                let sync = match &out.backend {
                    Backend::DeckLink(d) => d.reference().map(|r| pmg_proto::SyncState {
                        present: r.present,
                        locked: r.locked,
                        format: r.format,
                    }),
                    // NDI and headless have no reference input. None means "no
                    // such concept", which is NOT the same as present: false
                    // and must not be shown as an unlocked card.
                    _ => None,
                };
                // Two clocks being held together is a thing an operator has to
                // be able to SEE. Underruns are audible and corrections that
                // climb without settling mean something else is wrong.
                let audio_out = out.audio_sink.as_ref().map(|s| pmg_proto::AudioOutState {
                    device: s.device_name().to_string(),
                    host: s.host_name().to_string(),
                    buffered: s.buffered(),
                    underruns: s.underruns(),
                    dropped: s.dropped(),
                    corrections: out.async_.as_ref().map(|a| a.corrections).unwrap_or(0),
                    resyncs: out.async_.as_ref().map(|a| a.resyncs).unwrap_or(0),
                });
                if let Ok(mut e) = engine.lock() {
                    if let Some(o) = e.outputs.iter_mut().find(|o| o.id == out.spec.id) {
                        o.sync = sync;
                        o.audio_out = audio_out;
                        // Idempotent, and repeated on purpose: the engine
                        // recreates outputs on every assignment, and this poll
                        // is the place that already reconciles per-output
                        // facts the assignment cannot know.
                        o.preview_port = if out.preview.is_some() || out.preview_h264.is_some() {
                            out.spec.preview_port
                        } else {
                            0
                        };
                    }
                }
            }
        }
    }
    Ok(())
}

fn open_backend(spec: &OutputSpec) -> Result<Backend> {
    match spec.backend.as_str() {
        "headless" => Ok(Backend::Headless),
        "decklink" => {
            let s = pmg_out_decklink::Sender::open(&pmg_out_decklink::Config {
                device_index: spec.decklink_device,
                key_device_index: spec.decklink_key_device,
                mode: spec.decklink_mode.clone(),
                key_mode: pmg_out_decklink::KeyMode::FillOnly,
                audio_channels: spec.audio_channels,
                preroll: 3,
            })?;
            let (w, h) = s.size();
            if (w, h) != (spec.width, spec.height) {
                // Refuse rather than scale. A silently letterboxed graphic is a
                // worse discovery than an output that would not start.
                anyhow::bail!(
                    "{}: the display mode is {w}x{h} but this output renders {}x{}",
                    spec.id,
                    spec.width,
                    spec.height
                );
            }
            tracing::info!(output = %spec.id, device = spec.decklink_device,
                           mode = %spec.decklink_mode, key = ?spec.decklink_key_device,
                           "DeckLink output open");
            Ok(Backend::DeckLink(Box::new(s)))
        }
        _ => Ok(Backend::Ndi(pmg_out_ndi::Sender::open(
            &spec.ndi_name,
            spec.width,
            spec.height,
            spec.fps.0 as i32,
            spec.fps.1 as i32,
        )?)),
    }
}

/// Send an ack to whoever asked for it.
///
/// A cue from the server is acked up the link; a cue from the node's own API on
/// :8520 is answered over its own channel and **not** sent up the link — the
/// server never asked for it and has no pending cue to match it to. Both go
/// through the identical apply path; only the reply address differs.
fn answer(
    tx: &Sender<ToLink>,
    reply: Option<std::sync::mpsc::Sender<pmg_proto::CueAck>>,
    ack: pmg_proto::CueAck,
) {
    match reply {
        Some(r) => {
            let _ = r.send(ack);
        }
        None => {
            let _ = tx.send(ToLink::Ack(ack));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_late;
    use std::time::Duration;

    #[test]
    fn a_cue_inside_its_deadline_is_not_late() {
        assert!(!is_late(Some(500), Duration::from_millis(0)));
        assert!(!is_late(Some(500), Duration::from_millis(499)));
        // Exactly on the deadline is still in time. A take that arrives at its
        // own deadline is a take that made it.
        assert!(!is_late(Some(500), Duration::from_millis(500)));
    }

    #[test]
    fn a_cue_past_its_deadline_is_late() {
        assert!(is_late(Some(500), Duration::from_millis(501)));
        // The case that actually happened: the node was busy staging a clip and
        // the cue sat in the channel while the server gave up waiting.
        assert!(is_late(Some(500), Duration::from_secs(2)));
    }

    #[test]
    fn an_op_with_no_deadline_is_never_late() {
        // An arm or a preload is not time-critical, and refusing one for being
        // slow would make a busy node un-preparable exactly when it needs to be.
        assert!(!is_late(None, Duration::from_secs(60)));
    }
}
