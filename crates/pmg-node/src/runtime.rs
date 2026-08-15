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

pub struct Options {
    pub output_id: String,
    pub layers: Vec<String>,
    pub fps: (u32, u32),
    pub ndi_name: String,
    /// `ndi` | `decklink`
    pub backend: String,
    pub decklink_device: usize,
    /// Second port for the key half of a fill+key pair.
    pub decklink_key_device: Option<usize>,
    pub decklink_mode: String,
    pub width: u32,
    pub height: u32,
    pub server_http: String,
    pub fallback_font: Option<Vec<u8>>,
    pub headless: bool,
}

pub struct Runtime {
    engine: LayerEngine,
    assigned: Assigned,
    store: Store,
    fetcher: Fetcher,
    opts: Options,
}

impl Runtime {
    pub fn new(store: Store, opts: Options) -> Self {
        let fetcher = Fetcher::new(&opts.server_http);
        let mut engine = LayerEngine::new();
        engine.configure(
            &[pmg_proto::OutputAssignment {
                id: opts.output_id.clone(),
                mode: format!("{}x{}", opts.width, opts.height),
                layers: opts.layers.clone(),
            }],
            opts.fps,
        );
        Self {
            engine,
            assigned: Assigned {
                templates: BTreeMap::new(),
                items: BTreeMap::new(),
                font_bytes: Vec::new(),
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
        self.engine.gen = a.gen;
        self.engine.configure(&a.outputs, self.opts.fps);

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

    /// The frame loop. Runs until the link channel closes.
    pub fn run(&mut self, rx: Receiver<FromLink>, tx: Sender<ToLink>) -> Result<()> {
        let gpu = Gpu::open()?;
        tracing::info!(adapter = %gpu.adapter_name, backend = %gpu.backend, "gpu");
        let target = Target::new(&gpu, self.opts.width, self.opts.height)?;

        let backend = if self.opts.headless {
            Backend::Headless
        } else if self.opts.backend == "decklink" {
            let s = pmg_out_decklink::Sender::open(&pmg_out_decklink::Config {
                device_index: self.opts.decklink_device,
                key_device_index: self.opts.decklink_key_device,
                mode: self.opts.decklink_mode.clone(),
                // Two ordinary outputs, driven in step by us — not the card's
                // hardware pairing. Works on any two SDI outs.
                key_mode: pmg_out_decklink::KeyMode::FillOnly,
                preroll: 3,
            })?;
            let (w, h) = s.size();
            if (w, h) != (self.opts.width, self.opts.height) {
                // Refuse rather than scale. A silently letterboxed graphic is a
                // worse discovery than a node that would not start.
                anyhow::bail!(
                    "the display mode is {w}x{h} but this node renders {}x{}. Set --size to match.",
                    self.opts.width,
                    self.opts.height
                );
            }
            tracing::info!(
                device = self.opts.decklink_device,
                mode = %self.opts.decklink_mode,
                key = ?self.opts.decklink_key_device,
                "DeckLink output open"
            );
            Backend::DeckLink(Box::new(s))
        } else {
            Backend::Ndi(pmg_out_ndi::Sender::open(
                &self.opts.ndi_name,
                self.opts.width,
                self.opts.height,
                self.opts.fps.0 as i32,
                self.opts.fps.1 as i32,
            )?)
        };

        // One renderer per occupied layer, kept across frames. See RendererCache
        // — a single shared renderer would be rebuilt once per layer per frame
        // the moment two layers are occupied.
        let mut renderers = crate::render::RendererCache::default();
        let mut last_state = Instant::now();
        let mut worst_ms = 0.0f64;
        let budget_ms = 1000.0 / (self.opts.fps.0 as f64 / self.opts.fps.1 as f64);
        let output_id = self.opts.output_id.clone();

        loop {
            // --- messages -------------------------------------------------
            loop {
                match rx.try_recv() {
                    Ok(FromLink::Connected { epoch }) => {
                        self.engine.set_epoch(epoch);
                        let _ = tx.send(ToLink::State(self.engine.state_report()));
                    }
                    Ok(FromLink::Tick) => {
                        let _ = tx.send(ToLink::State(self.engine.state_report()));
                    }
                    Ok(FromLink::Disconnected) => {
                        // Deliberately nothing. What is on air stays on air.
                        tracing::warn!("link down — still playing");
                    }
                    Ok(FromLink::Assignment(a)) => {
                        // Persist FIRST, then adopt, then ack. Acking before
                        // the fsync would mean claiming durability we do not
                        // have (ADR 0009).
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
                        let ack =
                            self.engine
                                .apply(&c, &self.assigned, crate::link::now_ms(), false);
                        tracing::info!(cue = %c.cue_id, result = ?ack.result, reason = ?ack.reason, "cue");
                        let _ = tx.send(ToLink::Ack(ack));
                        let _ = tx.send(ToLink::State(self.engine.state_report()));
                    }
                    Ok(FromLink::Resync(reason)) => {
                        tracing::warn!(%reason, "server asked for a resync");
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(()),
                }
            }

            // --- one frame -------------------------------------------------
            let t0 = Instant::now();

            // Composite the stack bottom to top. The FIRST occupied layer
            // clears; every layer above it composites over the result. Drawing
            // each with a clear — which is what the single-layer path did —
            // means only the topmost layer survives, and the symptom is a Bug
            // silently erasing the Lower Third beneath it.
            let mut live: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut drawn = 0usize;

            if let Some(out) = self.engine.outputs.iter().find(|o| o.id == output_id) {
                for layer in &out.layers {
                    let Some(occ) = &layer.occupant else { continue };
                    live.insert(layer.name.clone());
                    let key = format!("{}|{}", occ.armed.item_id, occ.armed.template_rev);
                    let renderer = renderers.get(&gpu, &layer.name, &key, &occ.armed.instance);
                    let values = engine::frame_values(occ);
                    let load = if drawn == 0 {
                        pmg_render::Load::Clear
                    } else {
                        pmg_render::Load::Over
                    };
                    crate::render::draw(
                        &gpu,
                        renderer,
                        &occ.armed.instance,
                        &values,
                        &target,
                        load,
                    );
                    drawn += 1;
                }
            }

            // Nothing on air: the output must still be transparent black rather
            // than the last frame that was drawn into it.
            if drawn == 0 {
                target.clear(&gpu);
            }
            // A cleared layer must not hold its atlas for the rest of the
            // service.
            renderers.retain(&live);

            let rgba = target.read(&gpu)?;
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            if ms > worst_ms {
                worst_ms = ms;
            }
            if ms > budget_ms {
                if let Some(o) = self.engine.outputs.iter_mut().find(|o| o.id == output_id) {
                    o.late_frames += 1;
                }
            }

            match &backend {
                // NDI was opened with clock_video, so this call is what paces
                // the loop. No timer of our own, and none wanted here.
                Backend::Ndi(n) => n.send_rgba_premultiplied(&rgba)?,
                // DeckLink schedules against its own clock and blocks when the
                // queue is full, which paces the loop the same way — but for a
                // different reason, and against a clock that can be genlocked.
                Backend::DeckLink(d) => d.send_rgba_premultiplied(&rgba)?,
                Backend::Headless => std::thread::sleep(Duration::from_millis(budget_ms as u64)),
            }

            self.engine.tick(&output_id);

            if last_state.elapsed() >= Duration::from_millis(500) {
                last_state = Instant::now();
                let _ = tx.send(ToLink::State(self.engine.state_report()));
            }
        }
    }

    pub fn engine(&self) -> &LayerEngine {
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
