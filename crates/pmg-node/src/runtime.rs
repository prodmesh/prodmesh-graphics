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
        let mut db = pmg_text::FontDb::new();
        let faces: Vec<&Vec<u8>> = if self.font_bytes.is_empty() {
            self.fallback_font.iter().collect()
        } else {
            self.font_bytes.iter().collect()
        };
        // Register each face under every family any staged template names, so
        // the slice runs with whatever face is available. Real family matching
        // arrives with font assets in M3; pretending to match now would hide
        // that this is provisional.
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
        for bytes in faces {
            for (family, weight) in &families {
                db.add_face(bytes.clone(), family.clone(), *weight)?;
            }
        }
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
        let mut blocked = Vec::new();
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
                    blocked.push(pmg_proto::BlockedItem {
                        item: t.template_rev.clone(),
                        missing: Vec::new(),
                        unreadable: Some(format!(
                            "needs reader {}, this node is {}",
                            doc.min_reader,
                            pmg_proto::SCENE_READER_MAJOR
                        )),
                    });
                }
                Ok(doc) => {
                    self.assigned.templates.insert(t.template_rev.clone(), doc);
                }
                Err(e) => {
                    tracing::warn!(rev = %t.template_rev, error = %e, "template not readable");
                    blocked.push(pmg_proto::BlockedItem {
                        item: t.template_rev.clone(),
                        missing: vec![t.template_rev.clone()],
                        unreadable: None,
                    });
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

        self.assigned.items.clear();
        let mut ready = Vec::new();
        for it in &a.items {
            let armable = self.assigned.templates.contains_key(&it.template_rev);
            self.assigned.items.insert(
                it.id.clone(),
                (it.label.clone(), it.template_rev.clone(), it.data.clone()),
            );
            if armable {
                ready.push(it.id.clone());
            } else {
                blocked.push(pmg_proto::BlockedItem {
                    item: it.id.clone(),
                    missing: vec![it.template_rev.clone()],
                    unreadable: None,
                });
            }
        }

        if let Some(tx) = tx {
            let _ = tx.send(ToLink::Ready(pmg_proto::Ready {
                assignment_gen: a.gen,
                items_ready: ready,
                items_blocked: blocked,
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
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut handles = Vec::new();

        for (name, specs) in groups {
            let engine = engine.clone();
            let stop = stop.clone();
            let group_name = name.clone();
            handles.push(
                std::thread::Builder::new()
                    .name(format!("pmg-out-{name}"))
                    .spawn(move || {
                        if let Err(e) = render_group(&group_name, &specs, engine, stop) {
                            // A dead output must be loud. It is also not a
                            // reason to take the others down.
                            tracing::error!(group = %group_name, error = %e, "render group stopped");
                        }
                    })?,
            );
        }

        let mut last_state = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(FromLink::Connected { epoch }) => {
                    self.engine.lock().unwrap().set_epoch(epoch);
                    let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
                }
                Ok(FromLink::Tick) => {
                    let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
                }
                Ok(FromLink::Disconnected) => {
                    // Deliberately nothing. What is on air stays on air.
                    tracing::warn!("link down — still playing");
                }
                Ok(FromLink::Assignment(a)) => {
                    // Persist FIRST, then adopt, then ack. Acking before the
                    // fsync would claim durability we do not have (ADR 0009).
                    match self.store.save_assignment(&a) {
                        Ok(()) => {
                            self.adopt(&a, Some(&tx));
                            let _ = tx.send(ToLink::AssignmentSaved { gen: a.gen });
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "could not persist assignment; NOT acking")
                        }
                    }
                }
                Ok(FromLink::Cue(c, received_at)) => {
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
                    let received_late = is_late(c.deadline_ms, received_at.elapsed());
                    let ack = {
                        let mut e = self.engine.lock().unwrap();
                        e.apply(&c, &self.assigned, crate::link::now_ms(), received_late)
                    };
                    tracing::info!(cue = %c.cue_id, result = ?ack.result, reason = ?ack.reason, "cue");
                    let _ = tx.send(ToLink::Ack(ack));
                    let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
                }
                Ok(FromLink::Resync(reason)) => {
                    tracing::warn!(%reason, "server asked for a resync");
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if last_state.elapsed() >= Duration::from_millis(500) {
                last_state = Instant::now();
                let _ = tx.send(ToLink::State(self.engine.lock().unwrap().state_report()));
            }
        }

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for h in handles {
            let _ = h.join();
        }
        Ok(())
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
fn is_late(deadline_ms: Option<u64>, elapsed: std::time::Duration) -> bool {
    deadline_ms.is_some_and(|d| elapsed.as_millis() as u64 > d)
}

/// What one layer needs drawn this frame, taken under the lock and used without
/// it.
struct LayerPlan {
    layer: String,
    key: String,
    inst: Arc<pmg_render::scene::SceneInstance>,
    values: std::collections::BTreeMap<String, f32>,
    players: Option<Arc<std::sync::Mutex<engine::PlayerSet>>>,
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
        /// `None` when this output carries no audio.
        aclock: Option<pmg_audio::AudioClock>,
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
            audio: Vec::new(),
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

            // --- snapshot, under the lock, briefly ------------------------
            let (plan, pattern) = {
                let e = engine.lock().unwrap();
                let Some(o) = e.outputs.iter().find(|o| o.id == out.spec.id) else {
                    continue;
                };
                let plan: Vec<LayerPlan> = o
                    .layers
                    .iter()
                    .filter_map(|l| {
                        l.occupant.as_ref().map(|occ| LayerPlan {
                            layer: l.name.clone(),
                            key: format!("{}|{}", occ.armed.item_id, occ.armed.template_rev),
                            inst: occ.armed.instance.clone(),
                            values: engine::frame_values(occ),
                            // An Arc clone, so the players come out of the lock
                            // with everything else and are touched outside it.
                            players: occ.armed.players.clone(),
                        })
                    })
                    .collect();
                (plan, o.test_pattern.clone())
            };

            // --- draw, with the lock released -----------------------------
            // This frame's audio buffer, sized by the OUTPUT's clock. Cleared
            // and refilled rather than reallocated — a service must not
            // allocate a buffer a frame.
            let asamples = out
                .aclock
                .map(|c| c.samples_for(out.frame) as usize)
                .unwrap_or(0);
            out.audio.clear();
            out.audio
                .resize(asamples * out.spec.audio_channels.max(0) as usize, 0);

            let mut live = std::collections::HashSet::new();
            for (i, p) in plan.iter().enumerate() {
                live.insert(p.layer.clone());
                let renderer = out.renderers.get(&gpu, &p.layer, &p.key, &p.inst);
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
                crate::render::draw(&gpu, renderer, &p.inst, &p.values, &out.target, load);
            }
            if plan.is_empty() {
                out.target.clear(&gpu);
            }
            out.renderers.retain(&live);

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

            match &out.backend {
                // NDI is opened with clock_video, so this call paces the loop.
                Backend::Ndi(n) => {
                    n.send_rgba_premultiplied(&rgba)?;
                    // Audio AFTER the picture, so a video failure never leaves
                    // a receiver holding sound for a frame that will not come.
                    n.send_audio_interleaved_32s(
                        &out.audio,
                        out.spec.audio_channels.max(0) as usize,
                    )?;
                }
                // DeckLink schedules against its own clock and blocks when the
                // queue is full — the same pacing, against a clock that can be
                // genlocked.
                Backend::DeckLink(d) => d.send_rgba_premultiplied_with_audio(&rgba, &out.audio)?,
                Backend::Headless => std::thread::sleep(Duration::from_millis(budget_ms as u64)),
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
                if let Ok(mut e) = engine.lock() {
                    if let Some(o) = e.outputs.iter_mut().find(|o| o.id == out.spec.id) {
                        o.sync = sync;
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
