//! The layer engine — the authority on what is on air.
//!
//! Everything else in this crate exists to feed this. The server is
//! authoritative for *content and assignment*; this struct is authoritative for
//! *what is showing*, and it answers `StateReport` from its own state rather
//! than from anything the server believes (ADR 0009).
//!
//! # The arm/take split is the whole performance story
//!
//! Arming does layout, binding, shaping, rasterization, and atlas packing —
//! 0.3–2 ms, once. A frame evaluates timelines and applies transforms, and
//! nothing else. `docs/INTEGRATION-NOTES.md` records what happens when you
//! ignore that: 17.55 ms against a 16.68 ms budget, i.e. a missed frame every
//! frame.
//!
//! # Cue rules that are not negotiable
//!
//! - A cue whose `seq <= last_applied_seq` is **deduped**, not reapplied.
//! - A cue whose deadline has passed is **rejected, not fired late**. A graphic
//!   four seconds after the button is worse than one that never appears.
//! - A non-null `at` is **rejected as unsupported** in v1 rather than ignored.
//!   A scheduled take that silently fires immediately is the worst outcome,
//!   because nobody finds out.

use anyhow::Result;
use pmg_proto::{Cue, CueAck, CueOp, CueResult, DataValue, RejectReason};
use pmg_render::scene::SceneInstance;
use pmg_render::text_source::TextEngineGlyphs;
use pmg_render::timeline;
use pmg_scene::doc::SceneDoc;
use pmg_text::{Measurer, TextEngine};
use std::collections::BTreeMap;

/// A template the node can read, plus its timelines.
pub struct Armed {
    pub item_id: String,
    pub item_label: String,
    pub template_rev: String,
    pub instance: SceneInstance,
    pub in_ms: f64,
    pub out_ms: f64,
    pub doc: SceneDoc,
}

/// Where a layer's occupant is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    In,
    Hold,
    Out,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::In => "in",
            Phase::Hold => "hold",
            Phase::Out => "out",
        }
    }
}

pub struct Occupant {
    pub armed: Armed,
    pub phase: Phase,
    /// Milliseconds since this phase began.
    pub t_ms: f64,
}

#[derive(Default)]
pub struct Layer {
    pub name: String,
    pub occupant: Option<Occupant>,
}

pub struct Output {
    pub id: String,
    pub layers: Vec<Layer>,
    /// Colour handling, from the assignment.
    pub color: pmg_proto::ColorConfig,
    /// A test pattern overriding everything on this output, if one is up.
    ///
    /// It sits ABOVE the layer stack on purpose: an instrument you have to
    /// clear the show off air to see is an instrument nobody uses during a
    /// service.
    pub test_pattern: Option<String>,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frame: u64,
    pub late_frames: u64,
}

impl Output {
    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }

    fn layer_mut(&mut self, name: &str) -> Option<&mut Layer> {
        self.layers.iter_mut().find(|l| l.name == name)
    }
}

/// What the engine needs in order to arm an item: the compiled template and the
/// fonts it names. Supplied by the caller so this module needs no filesystem.
pub trait Library {
    /// The compiled SceneDoc for a revision, or `None` if it is not staged.
    fn template(&self, template_rev: &str) -> Option<&SceneDoc>;
    /// The Item's label and bound data.
    fn item(&self, item_id: &str) -> Option<(&str, &str, &BTreeMap<String, DataValue>)>;
    /// A text engine primed with the fonts the assignment carried.
    fn text_engine(&self) -> Result<TextEngine>;

    /// Decoded pixels for an asset key the document names, if it is staged.
    ///
    /// Returning `None` marks the scene degraded rather than drawing nothing:
    /// an empty rectangle where a background should be is the failure that gets
    /// noticed on air instead of before it.
    fn image(&self, asset_key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)>;
}

pub struct LayerEngine {
    pub outputs: Vec<Output>,
    pub last_applied_seq: u64,
    pub gen: u64,
    /// Cue ids already applied this epoch. This is the dedupe that makes a
    /// double-click, a bouncing GPI contact, and an HTTP retry all safe.
    applied: std::collections::HashSet<String>,
    pub epoch: u64,
}

impl LayerEngine {
    pub fn new() -> Self {
        Self {
            outputs: Vec::new(),
            last_applied_seq: 0,
            gen: 0,
            applied: std::collections::HashSet::new(),
            epoch: 0,
        }
    }

    /// A new server epoch invalidates the idempotency keys, because `seq`
    /// restarts and the server may reuse numbers it already used.
    pub fn set_epoch(&mut self, epoch: u64) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.applied.clear();
            self.last_applied_seq = 0;
        }
    }

    /// Apply an output/layer configuration.
    ///
    /// **The assignment's order is authoritative**, because layer order IS
    /// compositing order — bottom first. But reconfiguring must not evict: a
    /// graphic already on air stays on air, it just ends up at its new position
    /// in the stack.
    ///
    /// The first version of this appended unknown layers to whatever the node
    /// already had, which preserved occupants and silently produced the wrong
    /// stack order. A node booted with `Lower Third` and then told
    /// `Fullscreen, Lower Third, Bug` ended up compositing
    /// `Lower Third, Fullscreen, Bug` — so a fullscreen graphic would have
    /// covered the lower third that was supposed to be on top of it.
    pub fn configure(&mut self, outputs: &[pmg_proto::OutputAssignment], fps: (u32, u32)) {
        for oa in outputs {
            match self.outputs.iter_mut().find(|o| o.id == oa.id) {
                Some(existing) => {
                    existing.color = oa.color.clone();
                    // Rebuild in the assignment's order, carrying occupants
                    // across by NAME. A layer that disappears from the stack
                    // takes its occupant with it, which is the honest reading
                    // of "this output no longer has that layer".
                    let mut carried: std::collections::HashMap<String, Option<Occupant>> = existing
                        .layers
                        .drain(..)
                        .map(|l| (l.name, l.occupant))
                        .collect();
                    existing.layers = oa
                        .layers
                        .iter()
                        .map(|name| Layer {
                            name: name.clone(),
                            occupant: carried.remove(name).flatten(),
                        })
                        .collect();
                }
                None => self.outputs.push(Output {
                    id: oa.id.clone(),
                    color: oa.color.clone(),
                    test_pattern: None,
                    layers: oa
                        .layers
                        .iter()
                        .map(|n| Layer {
                            name: n.clone(),
                            occupant: None,
                        })
                        .collect(),
                    fps_num: fps.0,
                    fps_den: fps.1,
                    frame: 0,
                    late_frames: 0,
                }),
            }
        }
    }

    fn output_mut(&mut self, id: &str) -> Option<&mut Output> {
        self.outputs.iter_mut().find(|o| o.id == id)
    }

    /// Apply a cue and produce its ack.
    ///
    /// Every path here returns an ack. A cue the node silently drops is the one
    /// failure the whole protocol exists to prevent.
    pub fn apply(
        &mut self,
        cue: &Cue,
        lib: &dyn Library,
        now_ms: u64,
        received_late: bool,
    ) -> CueAck {
        let ack = |result, reason, frame| CueAck {
            seq: cue.seq,
            cue_id: cue.cue_id.clone(),
            result,
            reason,
            fired_at_frame: frame,
            at_utc: now_ms,
        };

        // Reserved, and refused rather than ignored (pmg-link.md §5.5).
        if cue.at.is_some() {
            return ack(CueResult::Rejected, Some(RejectReason::Unsupported), None);
        }

        // Idempotency, on both keys. `seq` catches a replay after a reconnect;
        // `cue_id` catches the same human action arriving twice by two routes.
        if cue.seq <= self.last_applied_seq || self.applied.contains(&cue.cue_id) {
            return ack(CueResult::Deduped, None, None);
        }

        // Late is rejected, never fired. This is also what kills a zombie cue
        // that arrives after a twenty-second reconnect.
        if received_late && cue.op.is_deadline_bearing() {
            return ack(
                CueResult::Rejected,
                Some(RejectReason::DeadlinePassed),
                None,
            );
        }

        let result = self.dispatch(&cue.op, lib);
        match result {
            Ok(frame) => {
                self.last_applied_seq = cue.seq;
                self.applied.insert(cue.cue_id.clone());
                ack(CueResult::Applied, None, frame)
            }
            Err(reason) => ack(CueResult::Rejected, Some(reason), None),
        }
    }

    fn dispatch(&mut self, op: &CueOp, lib: &dyn Library) -> Result<Option<u64>, RejectReason> {
        match op {
            CueOp::Take {
                item,
                output,
                layer,
            }
            | CueOp::Arm {
                item,
                output,
                layer,
            } => {
                let armed = self.arm(item, lib)?;
                let taking = matches!(op, CueOp::Take { .. });
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                let l = out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                if taking {
                    l.occupant = Some(Occupant {
                        armed,
                        phase: Phase::In,
                        t_ms: 0.0,
                    });
                    Ok(Some(frame))
                } else {
                    // Arm builds the instance and throws it away in M1: there
                    // is no preview slot yet. The cost it proves is real, and
                    // pretending to hold it would be worse than not having it.
                    Ok(None)
                }
            }

            CueOp::TakeOut { output, layer } => {
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                let l = out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                match &mut l.occupant {
                    Some(o) => {
                        o.phase = Phase::Out;
                        o.t_ms = 0.0;
                        Ok(Some(frame))
                    }
                    // Taking out an empty layer is success, not an error: the
                    // operator asked for it to be empty and it is empty.
                    None => Ok(Some(frame)),
                }
            }

            CueOp::ClearLayer { output, layer } => {
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                out.layer_mut(layer)
                    .ok_or(RejectReason::NoSuchLayer)?
                    .occupant = None;
                Ok(Some(frame))
            }

            CueOp::ClearAll { output } => {
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                for l in &mut out.layers {
                    l.occupant = None;
                }
                Ok(Some(frame))
            }

            CueOp::Update { output, layer, .. } => {
                // Rebinding a live instance needs a relayout, which M1's engine
                // does not do without re-arming. Refusing is honest; silently
                // showing stale text would not be.
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                Err(RejectReason::Unsupported)
            }

            CueOp::Timeline {
                output,
                layer,
                name,
                action,
            } => {
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                let l = out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                let o = l.occupant.as_mut().ok_or(RejectReason::NotReady)?;
                match action {
                    pmg_proto::TimelineAction::Play => {
                        o.phase = if name == "Out" { Phase::Out } else { Phase::In };
                        o.t_ms = 0.0;
                    }
                    pmg_proto::TimelineAction::Stop => o.phase = Phase::Hold,
                    pmg_proto::TimelineAction::Goto { t_ms } => o.t_ms = *t_ms as f64,
                }
                Ok(Some(frame))
            }

            CueOp::Preload { item } => {
                self.arm(item, lib)?;
                Ok(None)
            }

            CueOp::TestPattern { output, pattern } => {
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                // Validated here rather than drawn as a magenta placeholder: a
                // test pattern nobody recognises is worse than an error,
                // because the whole point is to trust what you are looking at.
                if let Some(p) = pattern {
                    if pmg_color::patterns::Pattern::parse(p).is_none() {
                        return Err(RejectReason::Unsupported);
                    }
                }
                out.test_pattern = pattern.clone();
                Ok(Some(frame))
            }
        }
    }

    /// Build a `SceneInstance` for an item. This is the expensive half.
    fn arm(&self, item_id: &str, lib: &dyn Library) -> Result<Armed, RejectReason> {
        let (label, template_rev, data) = lib.item(item_id).ok_or(RejectReason::NoSuchItem)?;
        let doc = lib.template(template_rev).ok_or(RejectReason::NotReady)?;

        // Refused HERE only as a backstop. The real refusal happens at
        // assignment time, loudly, so you find out Saturday and not at 10:15
        // Sunday (pmg-link.md §4).
        if doc.min_reader > pmg_proto::SCENE_READER_MAJOR {
            return Err(RejectReason::UnreadableTemplate);
        }

        let bound: pmg_scene::bind::Data = data
            .iter()
            .map(|(k, v)| (k.clone(), v.as_binding_str()))
            .collect();

        let engine = lib.text_engine().map_err(|_| RejectReason::NotReady)?;
        let measurer = Measurer::new(engine);
        let solved = pmg_layout::solve(doc, &bound, &measurer)
            .map_err(|_| RejectReason::UnreadableTemplate)?;
        let mut engine = measurer.into_engine();
        // The library owns decoded pixels; this adapter just hands them over,
        // so the renderer still knows nothing about decoding or the CAS.
        struct Images<'a>(&'a dyn Library);
        impl pmg_render::scene::ImageSource for Images<'_> {
            fn image(&mut self, key: &str) -> Option<(u32, u32, std::sync::Arc<Vec<u8>>)> {
                self.0.image(key)
            }
        }
        let instance = SceneInstance::build_with(
            doc,
            &solved,
            &mut TextEngineGlyphs::new(&mut engine),
            &mut Images(lib),
        );

        Ok(Armed {
            item_id: item_id.to_string(),
            item_label: label.to_string(),
            template_rev: template_rev.to_string(),
            in_ms: timeline::find(doc, "In")
                .map(|t| t.duration_ms as f64)
                .unwrap_or(0.0),
            out_ms: timeline::find(doc, "Out")
                .map(|t| t.duration_ms as f64)
                .unwrap_or(0.0),
            instance,
            doc: doc.clone(),
        })
    }

    /// Advance every layer by one frame and evict anything whose Out finished.
    pub fn tick(&mut self, output_id: &str) {
        let Some(out) = self.outputs.iter_mut().find(|o| o.id == output_id) else {
            return;
        };
        let dt = 1000.0 / (out.fps_num as f64 / out.fps_den as f64);
        out.frame += 1;

        for layer in &mut out.layers {
            let Some(o) = &mut layer.occupant else {
                continue;
            };
            o.t_ms += dt;
            match o.phase {
                // In runs to its end and then HOLDS at that end state. It is
                // not re-evaluated from zero, which would restart the animation
                // every frame.
                Phase::In if o.t_ms >= o.armed.in_ms => {
                    o.phase = Phase::Hold;
                    o.t_ms = 0.0;
                }
                Phase::Out if o.t_ms >= o.armed.out_ms => {
                    layer.occupant = None;
                }
                _ => {}
            }
        }
    }

    pub fn state_report(&self) -> pmg_proto::StateReport {
        pmg_proto::StateReport {
            gen: self.gen,
            last_applied_seq: self.last_applied_seq,
            outputs: self
                .outputs
                .iter()
                .map(|o| pmg_proto::OutputState {
                    id: o.id.clone(),
                    fps: o.fps() as f32,
                    late_frames: o.late_frames,
                    layers: o
                        .layers
                        .iter()
                        .map(|l| pmg_proto::LayerState {
                            name: l.name.clone(),
                            occupant: l.occupant.as_ref().map(|o| pmg_proto::Occupant {
                                item_id: o.armed.item_id.clone(),
                                item_label: o.armed.item_label.clone(),
                                template_rev: o.armed.template_rev.clone(),
                                phase: o.phase.as_str().to_string(),
                                timeline: Some(
                                    match o.phase {
                                        Phase::Out => "Out",
                                        _ => "In",
                                    }
                                    .to_string(),
                                ),
                                t_ms: o.t_ms as u64,
                                // On air AND wrong is a thing the operator must
                                // be told; staying quiet is the failure.
                                degraded: o.armed.instance.degraded,
                            }),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl Default for LayerEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// The timeline values for a layer at its current moment.
pub fn frame_values(o: &Occupant) -> std::collections::BTreeMap<String, f32> {
    let (name, t) = match o.phase {
        Phase::In => ("In", o.t_ms),
        // Hold is the In timeline's FINAL state, held.
        Phase::Hold => ("In", o.armed.in_ms),
        Phase::Out => ("Out", o.t_ms),
    };
    timeline::find(&o.armed.doc, name)
        .map(|tl| timeline::evaluate(tl, t as f32))
        .unwrap_or_default()
}
