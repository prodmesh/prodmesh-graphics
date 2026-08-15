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
pub enum Backend {
    /// Render and time frames, send nothing.
    Headless,
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

        // Decode every image a staged template names. Once, here, rather than
        // per arm.
        self.assigned.images.clear();
        for doc in self.assigned.templates.values() {
            for asset in &doc.assets {
                if self.assigned.images.contains_key(&asset.key) {
                    continue;
                }
                let Ok(bytes) = self.store.cas.get(&asset.hash) else {
                    continue;
                };
                match pmg_media::decode_image(&bytes) {
                    Ok(d) => {
                        self.assigned.images.insert(
                            asset.key.clone(),
                            (d.width, d.height, std::sync::Arc::new(d.rgba)),
                        );
                    }
                    Err(e) => {
                        // Named, with the key, because "the background did not
                        // appear" is otherwise unattributable.
                        tracing::warn!(key = %asset.key, hash = %asset.hash, error = %e,
                                       "could not decode an image asset");
                    }
                }
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
                Ok(FromLink::Cue(c)) => {
                    // Arming happens under the engine lock, which briefly stalls
                    // every render thread. An arm is 0.3–2 ms against a 16.68 ms
                    // budget and a take is a few times a minute, so it costs at
                    // most one late frame on a take — measured, not assumed, in
                    // the M3c notes. Moving the arm outside the lock is the
                    // obvious next step if that ever shows up on air.
                    let ack = {
                        let mut e = self.engine.lock().unwrap();
                        e.apply(&c, &self.assigned, crate::link::now_ms(), false)
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
        frame: u64,
    }

    let mut outputs = Vec::new();
    for spec in specs {
        let target = Target::new(&gpu, spec.width, spec.height)?;
        let backend = open_backend(spec)?;
        let preview = if spec.preview_port == 0 {
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
            preview,
            frame: 0,
        });
    }

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
                let plan: Vec<(String, String, Arc<pmg_render::scene::SceneInstance>, _)> = o
                    .layers
                    .iter()
                    .filter_map(|l| {
                        l.occupant.as_ref().map(|occ| {
                            (
                                l.name.clone(),
                                format!("{}|{}", occ.armed.item_id, occ.armed.template_rev),
                                occ.armed.instance.clone(),
                                engine::frame_values(occ),
                            )
                        })
                    })
                    .collect();
                (plan, o.test_pattern.clone())
            };

            // --- draw, with the lock released -----------------------------
            let mut live = std::collections::HashSet::new();
            for (i, (layer, key, inst, values)) in plan.iter().enumerate() {
                live.insert(layer.clone());
                let renderer = out.renderers.get(&gpu, layer, key, inst);
                let load = if i == 0 {
                    pmg_render::Load::Clear
                } else {
                    pmg_render::Load::Over
                };
                crate::render::draw(&gpu, renderer, inst, values, &out.target, load);
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
                Backend::Ndi(n) => n.send_rgba_premultiplied(&rgba)?,
                // DeckLink schedules against its own clock and blocks when the
                // queue is full — the same pacing, against a clock that can be
                // genlocked.
                Backend::DeckLink(d) => d.send_rgba_premultiplied(&rgba)?,
                Backend::Headless => std::thread::sleep(Duration::from_millis(budget_ms as u64)),
            }

            if let Some(p) = &out.preview {
                p.offer(&rgba);
            }
        }

        // Tick every output in this group together, which is what sharing a
        // clock means: their timelines advance on the same beat.
        if let Ok(mut e) = engine.lock() {
            for out in &outputs {
                e.tick(&out.spec.id);
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
