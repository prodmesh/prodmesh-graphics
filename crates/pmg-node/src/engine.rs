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

/// The clips one armed item is playing, and where its clock started.
///
/// Behind a mutex because the players are **mutable per frame** while the
/// instance beside them is immutable and shared. Exactly one thread ever touches
/// a given set — a layer belongs to one output, an output to one group, a group
/// to one render thread (ADR 0011) — so the lock is uncontended and exists to
/// satisfy the borrow checker rather than to arbitrate anything.
#[derive(Default)]
pub struct PlayerSet {
    /// One player per [`pmg_render::scene::VideoSlot`], with the image slot it
    /// feeds.
    pub players: Vec<(usize, pmg_media::video::VideoPlayer)>,
    /// Audio for the same clips, when they have any.
    ///
    /// Separate from `players` because a clip having sound is independent of it
    /// having pictures: a silent background loop is normal, and so is a stinger
    /// whose whole point is the sound.
    pub audio: Vec<pmg_media::audio::AudioDecoder>,
    /// The output frame the clip's frame 0 was shown on.
    ///
    /// Deliberately **not** the frame of the take. ffmpeg needs ~120 ms to
    /// produce its first picture, and starting the clock at the take would mean
    /// arriving 7 frames behind and then discarding those 7 to catch up — the
    /// clip would begin a fifth of a second in, having frozen on its first frame
    /// to get there. Starting the clock when the first frame actually lands
    /// costs the same wait and plays the whole clip.
    pub start_frame: Option<u64>,
}

/// A template the node can read, plus its timelines.
pub struct Armed {
    pub item_id: String,
    pub item_label: String,
    pub template_rev: String,
    /// Shared, so a render thread can take a reference and draw **without
    /// holding the engine lock**. With several output groups each on their own
    /// thread (ADR 0011), holding the lock across a draw would serialise them
    /// into one — and arming under that same lock would stall every output for
    /// the 0.3–2 ms an arm costs.
    pub instance: std::sync::Arc<SceneInstance>,
    /// The clips this item plays, or `None` when it has none — which is most
    /// items, and the reason this is an `Option` rather than an empty `Vec`
    /// behind a lock nobody needs to take.
    pub players: Option<std::sync::Arc<std::sync::Mutex<PlayerSet>>>,
    pub in_ms: f64,
    pub out_ms: f64,
    pub doc: SceneDoc,
    /// The Item's timer fields, by key (ADR 0022). Empty for almost every Item,
    /// which is why the per-frame cost of having timers at all is one `is_empty`.
    pub timers: Vec<(String, pmg_scene::timer::TimerSpec)>,
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

/// A live rebind in progress: what was on screen, and how far through.
///
/// Holding the OLD instance is what makes the morph possible — both ends were
/// solved, and every frame in between is arithmetic on rectangles rather than a
/// third layout (`pmg_render::tween`).
#[derive(Clone)]
pub struct Tween {
    pub from: std::sync::Arc<SceneInstance>,
    pub t_ms: f64,
    pub duration_ms: f64,
}

impl Tween {
    /// Eased progress, 0 to 1.
    pub fn t(&self) -> f32 {
        if self.duration_ms <= 0.0 {
            return 1.0;
        }
        pmg_render::tween::ease((self.t_ms / self.duration_ms) as f32)
    }

    pub fn done(&self) -> bool {
        self.t_ms >= self.duration_ms
    }
}

/// A named timeline in flight — `Update`, `Highlight`, whatever the designer
/// called it — layered over the occupant's base phase.
///
/// Runs live in start order and the last writer per target wins each frame
/// (`docs/specs/scene-format.md` §6.3). A finished run whose `on_complete` is
/// `Hold` STAYS in the list asserting its final values — that is what Hold
/// means — so the way back to the base look is an explicit `Stop`, or a
/// timeline authored to end where it began.
pub struct NamedRun {
    pub name: String,
    /// Milliseconds since this run started (wraps when the timeline loops).
    pub t_ms: f64,
    /// Still advancing. `false` is finished-and-holding.
    pub running: bool,
}

pub struct Occupant {
    pub armed: Armed,
    pub phase: Phase,
    /// Milliseconds since this phase began.
    pub t_ms: f64,
    /// A rebind morphing to a new layout, if one is running.
    pub tween: Option<Tween>,
    /// Named timelines playing (or holding) over the base phase, start order.
    pub named: Vec<NamedRun>,
    /// When this went on air, in UTC milliseconds — what `elapsed` counts from.
    pub taken_at_ms: i64,
    /// What each timer field currently READS. The comparison that decides
    /// whether a tick needs a rebind is on the string, not on the time: a clock
    /// showing minutes changes 60 times less often than one showing seconds,
    /// and neither should re-solve a layout for a number that has not changed.
    pub shown: BTreeMap<String, String>,
    /// UTC milliseconds at which `shown` next goes stale, or `None` when
    /// nothing here changes on its own.
    pub timer_due_ms: Option<i64>,
}

#[derive(Default)]
pub struct Layer {
    pub name: String,
    pub occupant: Option<Occupant>,
    /// The graphic this layer is dissolving out of, still playing its `Out`.
    ///
    /// A take over an occupied layer used to drop what was there on the same
    /// frame, so slide-to-slide was a hard cut plus an entrance. A cross-fade
    /// is just both graphics on screen at once — the old one running the `Out`
    /// its template already authors, the new one running its `In` — so this
    /// needs no new animation machinery, only somewhere to keep the old one.
    ///
    /// Retired the moment its `Out` finishes, so the steady state is one
    /// `Option` check rather than a second draw pass forever.
    pub outgoing: Option<Occupant>,
}

pub struct Output {
    pub id: String,
    pub layers: Vec<Layer>,
    /// Colour handling, from the assignment.
    pub color: pmg_proto::ColorConfig,
    /// Reference status, refreshed by the render thread that owns this output.
    pub sync: Option<pmg_proto::SyncState>,
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
    /// This output's preview port, filled in by the render thread that owns
    /// the output — the engine learns outputs from assignments, which do not
    /// know about preview transport.
    pub preview_port: u16,
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

    /// Where a clip lives on disk, and what it is.
    ///
    /// A **path**, not bytes: a service-length clip is gigabytes, and the whole
    /// point of decoding it a frame at a time is not to hold it in memory. The
    /// default is `None` so a library that has no video — every test one — needs
    /// to say nothing.
    fn video(&self, _asset_key: &str) -> Option<(std::path::PathBuf, pmg_media::video::VideoSpec)> {
        None
    }
}

pub struct LayerEngine {
    pub outputs: Vec<Output>,
    pub last_applied_seq: u64,
    pub gen: u64,
    /// Cue ids already applied this epoch. This is the dedupe that makes a
    /// double-click, a bouncing GPI contact, and an HTTP retry all safe.
    applied: std::collections::HashSet<String>,
    pub epoch: u64,
    /// Set once the node's own outputs are in place, after which the assignment
    /// may only configure them and never add to them.
    seeded: bool,
    /// This node's wall clock, in UTC milliseconds, and its offset from UTC —
    /// what timers read (ADR 0022).
    ///
    /// Held here rather than read from the operating system inside the engine,
    /// for the reason every clock in this codebase is passed in: a countdown
    /// whose time comes from `SystemTime::now()` two calls deep cannot be
    /// TESTED, and a countdown nobody can test is a countdown nobody should
    /// put on a wall.
    now_ms: i64,
    tz_offset_ms: i64,
}

impl LayerEngine {
    pub fn new() -> Self {
        Self {
            outputs: Vec::new(),
            last_applied_seq: 0,
            gen: 0,
            applied: std::collections::HashSet::new(),
            epoch: 0,
            seeded: false,
            now_ms: 0,
            tz_offset_ms: 0,
        }
    }

    /// Tell the engine what time it is. The runtime does this every pass;
    /// a test does it by hand, which is the point.
    pub fn set_clock(&mut self, now_ms: i64, tz_offset_ms: i64) {
        self.now_ms = now_ms;
        self.tz_offset_ms = tz_offset_ms;
    }

    /// When the soonest timer anywhere on this node next changes what it says.
    ///
    /// The runtime waits on its socket for exactly this long instead of its
    /// usual poll, so a clock ticks ON the second rather than up to a couple of
    /// hundred milliseconds after it.
    pub fn next_timer_due_ms(&self) -> Option<i64> {
        self.outputs
            .iter()
            .flat_map(|o| o.layers.iter())
            .filter_map(|l| l.occupant.as_ref())
            .filter_map(|o| o.timer_due_ms)
            .min()
    }

    /// Re-evaluate every live timer and rebind the ones whose STRING changed.
    ///
    /// Returns the layers it rebound, for the log. Called every pass of the
    /// runtime loop: with no timers anywhere it is one `is_empty` per layer,
    /// and with timers it is one integer comparison until the second turns
    /// over.
    ///
    /// **A timer rebind does not morph.** The tween exists so a lower third
    /// whose text changed mid-hold grows into its new shape; applied to a clock
    /// it would start a 400 ms animation on every tick and the next tick would
    /// interrupt it (ADR 0022).
    pub fn run_timers(&mut self, lib: &dyn Library, now_ms: i64, tz_offset_ms: i64) -> Vec<String> {
        self.set_clock(now_ms, tz_offset_ms);
        let mut done = Vec::new();

        // Which layers need what, decided before anything is armed: arming
        // borrows the engine immutably and the occupant is borrowed mutably.
        struct Due {
            output: String,
            layer: String,
            item: String,
            fps: (u32, u32),
            taken_at_ms: i64,
            values: BTreeMap<String, String>,
        }
        let mut due: Vec<Due> = Vec::new();
        for out in &self.outputs {
            for l in &out.layers {
                let Some(o) = &l.occupant else { continue };
                if o.armed.timers.is_empty() {
                    continue;
                }
                if o.timer_due_ms.is_some_and(|d| now_ms < d) {
                    continue;
                }
                let values: BTreeMap<String, String> = o
                    .armed
                    .timers
                    .iter()
                    .map(|(k, spec)| {
                        (
                            k.clone(),
                            pmg_scene::timer::format_at(spec, now_ms, tz_offset_ms, o.taken_at_ms),
                        )
                    })
                    .collect();
                if values == o.shown {
                    // Nothing to redraw; only the deadline moves.
                    continue;
                }
                due.push(Due {
                    output: out.id.clone(),
                    layer: l.name.clone(),
                    item: o.armed.item_id.clone(),
                    fps: (out.fps_num, out.fps_den),
                    taken_at_ms: o.taken_at_ms,
                    values,
                });
            }
        }

        for d in due {
            let overrides: BTreeMap<String, DataValue> = d
                .values
                .iter()
                .map(|(k, v)| (k.clone(), DataValue::Text(v.clone())))
                .collect();
            let Ok(rebound) = self.arm_inner_at(
                &d.item,
                lib,
                Some(d.fps),
                Some(&overrides),
                None,
                d.taken_at_ms,
            ) else {
                // The template or a font went away underneath a live graphic.
                // The old instance stays on screen showing the last number it
                // had, which is the least-wrong thing available: taking the
                // graphic down mid-service because a clock could not re-solve
                // would be a far bigger failure than a stopped clock.
                continue;
            };
            let Some(out) = self.output_mut(&d.output) else {
                continue;
            };
            let Some(l) = out.layer_mut(&d.layer) else {
                continue;
            };
            let Some(o) = l.occupant.as_mut() else {
                continue;
            };
            o.armed = rebound;
            o.shown = d.values;
            done.push(format!("{}/{}", d.output, d.layer));
        }

        // Deadlines last, for everything with timers — including the ones whose
        // string had not changed, whose deadline still has to move forward or
        // they would be recomputed every single pass.
        for out in &mut self.outputs {
            for l in &mut out.layers {
                let Some(o) = &mut l.occupant else { continue };
                if o.armed.timers.is_empty() {
                    o.timer_due_ms = None;
                    continue;
                }
                o.timer_due_ms = o
                    .armed
                    .timers
                    .iter()
                    .filter_map(|(_, spec)| {
                        pmg_scene::timer::ms_until_change(spec, now_ms, o.taken_at_ms)
                    })
                    .min()
                    .map(|ms| now_ms + ms.max(1));
            }
        }
        done
    }

    /// Declare the outputs this node physically drives.
    ///
    /// After this, `configure` may only adjust them — the server cannot invent
    /// an output that has no wire behind it.
    pub fn seal_outputs(&mut self) {
        self.seeded = true;
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
                            // A reconfigure does not carry a dissolve across.
                            // The layer stack changing mid-fade is rare and a
                            // half-faded ghost surviving it would be worse than
                            // the fade ending early.
                            outgoing: None,
                        })
                        .collect();
                }
                // An output this node does not physically have.
                //
                // **The node is authoritative about what outputs exist**;
                // capabilities are discovered, never configured. The server
                // assigns LAYERS to an output, it does not bring one into
                // being — and creating one here produced a phantom that
                // reported 59.94 fps and zero late frames with no render
                // thread behind it, which is worse than an error because it
                // looks healthy.
                None if self.seeded => {
                    tracing::warn!(
                        output = %oa.id,
                        "the assignment names an output this node does not have; ignoring it"
                    );
                }
                None => self.outputs.push(Output {
                    id: oa.id.clone(),
                    color: oa.color.clone(),
                    sync: None,
                    test_pattern: None,
                    layers: oa
                        .layers
                        .iter()
                        .map(|n| Layer {
                            name: n.clone(),
                            occupant: None,
                            outgoing: None,
                        })
                        .collect(),
                    fps_num: fps.0,
                    fps_den: fps.1,
                    frame: 0,
                    late_frames: 0,
                    preview_port: 0,
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
        // Every cue carries the wall time already, so a take knows when it
        // happened without asking the operating system from inside the engine.
        self.now_ms = now_ms as i64;
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
        //
        // **seq 0 means "not from the server's stream"** — a cue submitted to
        // the node's own API on :8520 (ADR 0014). It has no place in the
        // server's ordering, so the replay check does not apply to it, and
        // applying it anyway made EVERY local cue dedupe: `0 <= last_applied`
        // is true from the first frame, so a GPI could never fire anything.
        // Found by the M4 verification — a take with the server stopped.
        //
        // Dedupe still holds for these, on `cue_id`, which is the half of the
        // key a human action owns and the half that makes a bouncing contact
        // closure one take.
        let replayed = cue.seq > 0 && cue.seq <= self.last_applied_seq;
        if replayed || self.applied.contains(&cue.cue_id) {
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

    /// Ack a take that a newer take for the same layers replaced before it
    /// fired (ADR 0017).
    ///
    /// Called instead of `apply`, never as well as it — the whole point is that
    /// the layer does not flash through every slide the operator scrolled past.
    ///
    /// Dedupe still wins. A replay of a cue that already fired must say
    /// `Deduped`: it DID go on air, and calling that superseded would report a
    /// graphic as never shown while it was on the programme feed.
    pub fn supersede(&mut self, cue: &Cue, now_ms: u64) -> CueAck {
        let deduped = cue.seq <= self.last_applied_seq || self.applied.contains(&cue.cue_id);
        CueAck {
            seq: cue.seq,
            cue_id: cue.cue_id.clone(),
            result: if deduped {
                CueResult::Deduped
            } else {
                CueResult::Superseded
            },
            reason: None,
            fired_at_frame: None,
            at_utc: now_ms,
        }
    }

    fn dispatch(&mut self, op: &CueOp, lib: &dyn Library) -> Result<Option<u64>, RejectReason> {
        match op {
            // A take that fans out (ADR 0018).
            //
            // One operator action, several places on this node — a lower third
            // over the key feed and its full-screen version on another layer.
            // Applied as a unit: every destination is ARMED before any is put
            // on air, so a template that will not build cannot leave the key
            // feed live and the full-screen feed empty.
            // A take fires the AUTO destinations; TakeAlso fires the ones it
            // deliberately skipped (ADR 0018). Identical otherwise, so the two
            // share this arm and differ by one predicate — the alternative was
            // a second copy of the fan-out, which is where the two would drift.
            CueOp::Take {
                item, destinations, ..
            }
            | CueOp::TakeAlso {
                item, destinations, ..
            } if !destinations.is_empty() => {
                let wanted = matches!(op, CueOp::Take { .. });
                let mut prepared = Vec::new();
                for d in destinations.iter().filter(|d| d.is_auto() == wanted) {
                    let fps = self
                        .outputs
                        .iter()
                        .find(|o| o.id == d.output)
                        .map(|o| (o.fps_num, o.fps_den))
                        .ok_or(RejectReason::NoSuchLayer)?;
                    // A blank destination clears its layer rather than
                    // rendering. It is an instruction, so it is carried out —
                    // and it needs no template, which is why it is not armed.
                    let armed = if d.blank {
                        None
                    } else {
                        Some(self.arm_at(item, lib, Some(fps), &d.template_rev)?)
                    };
                    prepared.push((d.output.clone(), d.layer.clone(), armed));
                }
                if prepared.is_empty() {
                    // Nothing this op was asked to fire. For a take that means
                    // every destination is on_request; for a TakeAlso it means
                    // the Item has none. Either way, saying so beats reporting
                    // an action that put nothing on air.
                    return Err(RejectReason::NotReady);
                }

                let mut frame = None;
                for (output, layer, armed) in prepared {
                    let taken_at_ms = self.now_ms;
                    let out = self.output_mut(&output).ok_or(RejectReason::NoSuchLayer)?;
                    frame.get_or_insert(out.frame);
                    let l = out.layer_mut(&layer).ok_or(RejectReason::NoSuchLayer)?;
                    // Dissolve rather than cut. Whatever was here starts its
                    // Out now; the arriving graphic starts its In. Only one
                    // outgoing is kept — arrowing a song faster than the fade
                    // should replace the thing fading, not stack three deep.
                    l.outgoing = l.occupant.take().map(|mut o| {
                        o.phase = Phase::Out;
                        o.t_ms = 0.0;
                        // A morph in flight is abandoned: it was interpolating
                        // toward data this graphic is no longer showing.
                        o.tween = None;
                        // A flourish does not outlive the decision to leave —
                        // a held Highlight overriding Out's own opacity track
                        // would pin the graphic on screen while it "leaves".
                        o.named.clear();
                        o
                    });
                    l.occupant = armed.map(|armed| Occupant {
                        armed,
                        phase: Phase::In,
                        t_ms: 0.0,
                        tween: None,
                        named: Vec::new(),
                        // A graphic's `elapsed` counts from the moment it went
                        // on air, which is this one.
                        taken_at_ms,
                        shown: BTreeMap::new(),
                        // Due immediately: the first pass of the runtime loop
                        // after the take resolves every timer against the real
                        // clock and sets a real deadline.
                        timer_due_ms: Some(i64::MIN),
                    });
                }
                Ok(frame)
            }

            CueOp::Take {
                item,
                output,
                layer,
                ..
            }
            | CueOp::Arm {
                item,
                output,
                layer,
            } => {
                // The output's rate is needed BEFORE arming, because a clip's
                // clock is defined against it. Looked up first so a cue naming
                // an output that does not exist is refused without having paid
                // for a layout solve.
                let out_fps = self
                    .outputs
                    .iter()
                    .find(|o| &o.id == output)
                    .map(|o| (o.fps_num, o.fps_den))
                    .ok_or(RejectReason::NoSuchLayer)?;
                let armed = self.arm(item, lib, Some(out_fps))?;
                let taking = matches!(op, CueOp::Take { .. });
                let taken_at_ms = self.now_ms;
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                let l = out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                if taking {
                    l.occupant = Some(Occupant {
                        armed,
                        phase: Phase::In,
                        t_ms: 0.0,
                        tween: None,
                        named: Vec::new(),
                        // A graphic's `elapsed` counts from the moment it went
                        // on air, which is this one.
                        taken_at_ms,
                        shown: BTreeMap::new(),
                        // Due immediately: the first pass of the runtime loop
                        // after the take resolves every timer against the real
                        // clock and sets a real deadline.
                        timer_due_ms: Some(i64::MIN),
                    });
                    Ok(Some(frame))
                } else {
                    // Arm builds the instance and throws it away in M1: there
                    // is no preview slot yet. The cost it proves is real, and
                    // pretending to hold it would be worse than not having it.
                    Ok(None)
                }
            }

            // An Item with no destination list at all predates ADR 0018 and
            // therefore has nothing marked on_request. Refusing beats firing
            // its single destination a second time, which is what falling
            // through to Take would do.
            CueOp::TakeAlso { .. } => Err(RejectReason::NotReady),

            CueOp::TakeOut { output, layer } => {
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                let l = out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                match &mut l.occupant {
                    Some(o) => {
                        o.phase = Phase::Out;
                        o.t_ms = 0.0;
                        // See the take path: a held named run overriding Out's
                        // tracks would keep the graphic on screen while it
                        // "leaves".
                        o.named.clear();
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

            CueOp::Update {
                output,
                layer,
                data,
                tween_ms,
            } => {
                // Rebind a live Item and MORPH to the new layout.
                //
                // This returned Unsupported from M1 until 2026-08-16, honestly:
                // rebinding needs a relayout and the engine would not do one
                // without re-arming. It is now the feature the flexbox solver
                // was built for — a lower third whose box grows to fit new text
                // while it is on air, which is the one thing in an hour of real
                // service the current tooling cannot do
                // (`docs/reference/canyon-hills-service-rundown.md`).
                //
                // The arm/take split still holds. Solving here costs what an arm
                // costs, once, and every frame after it interpolates between two
                // finished layouts rather than solving a third.
                let out_id = output.clone();
                let occupied = self
                    .outputs
                    .iter()
                    .find(|o| &o.id == output)
                    .and_then(|o| o.layers.iter().find(|l| &l.name == layer))
                    .ok_or(RejectReason::NoSuchLayer)?
                    .occupant
                    .as_ref()
                    .map(|o| (o.armed.item_id.clone(), o.armed.template_rev.clone()));
                let Some((item_id, _)) = occupied else {
                    // Nothing to rebind. An Update to an empty layer is a
                    // mistake worth naming rather than a silent no-op.
                    return Err(RejectReason::NotReady);
                };

                let fps = self
                    .outputs
                    .iter()
                    .find(|o| o.id == out_id)
                    .map(|o| (o.fps_num, o.fps_den))
                    .ok_or(RejectReason::NoSuchLayer)?;
                let rebound = self.arm_with(&item_id, lib, Some(fps), Some(data))?;

                let ms = tween_ms.unwrap_or(pmg_proto::DEFAULT_TWEEN_MS) as f64;
                let out = self.output_mut(output).ok_or(RejectReason::NoSuchLayer)?;
                let frame = out.frame;
                let l = out.layer_mut(layer).ok_or(RejectReason::NoSuchLayer)?;
                let occ = l.occupant.as_mut().ok_or(RejectReason::NotReady)?;

                // A rebind arriving mid-rebind morphs from what is ON SCREEN
                // now, not from the state it was heading towards. Otherwise a
                // fast correction snaps backwards before moving forwards.
                let showing = occ
                    .tween
                    .as_ref()
                    .filter(|t| t.t_ms < t.duration_ms)
                    .map(|t| t.from.clone())
                    .unwrap_or_else(|| occ.armed.instance.clone());

                occ.tween = (ms > 0.0).then_some(Tween {
                    from: showing,
                    t_ms: 0.0,
                    duration_ms: ms,
                });
                occ.armed = rebound;
                Ok(Some(frame))
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

                // `In` and `Out` are reserved and drive the PHASE — the
                // occupant's life. Every other name is a run layered over it
                // (`docs/specs/scene-format.md` §6.1).
                if name == "In" || name == "Out" {
                    match action {
                        pmg_proto::TimelineAction::Play => {
                            o.phase = if name == "Out" { Phase::Out } else { Phase::In };
                            o.t_ms = 0.0;
                            if name == "Out" {
                                o.named.clear();
                            }
                        }
                        pmg_proto::TimelineAction::Stop => o.phase = Phase::Hold,
                        pmg_proto::TimelineAction::Goto { t_ms } => o.t_ms = *t_ms as f64,
                    }
                    return Ok(Some(frame));
                }

                // Refused BY NAME, so the operator learns which button is
                // wired to nothing — a graphic that silently does not move
                // teaches them to distrust every button on the panel.
                if timeline::find(&o.armed.doc, name).is_none() {
                    return Err(RejectReason::NoSuchTimeline);
                }
                match action {
                    // Play restarts a run already in flight rather than
                    // stacking a second copy — double-tapping Highlight should
                    // highlight NOW, not twice.
                    pmg_proto::TimelineAction::Play => {
                        match o.named.iter_mut().find(|r| &r.name == name) {
                            Some(run) => {
                                run.t_ms = 0.0;
                                run.running = true;
                            }
                            None => o.named.push(NamedRun {
                                name: name.clone(),
                                t_ms: 0.0,
                                running: true,
                            }),
                        }
                    }
                    // Stop REMOVES the run: its values vanish and the base
                    // phase shows through again. Freezing mid-pose would leave
                    // a stopped Loop shimmer half-lit with no way back.
                    pmg_proto::TimelineAction::Stop => o.named.retain(|r| &r.name != name),
                    pmg_proto::TimelineAction::Goto { t_ms } => {
                        match o.named.iter_mut().find(|r| &r.name == name) {
                            Some(run) => run.t_ms = *t_ms as f64,
                            None => o.named.push(NamedRun {
                                name: name.clone(),
                                t_ms: *t_ms as f64,
                                running: true,
                            }),
                        }
                    }
                }
                Ok(Some(frame))
            }

            CueOp::Preload { item } => {
                // `None` for the rate: a preload proves the item ARMS, and
                // opening decoders for a clip nobody has taken would leave an
                // ffmpeg running for every clip in the show.
                self.arm(item, lib, None)?;
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
    fn arm(
        &self,
        item_id: &str,
        lib: &dyn Library,
        out_fps: Option<(u32, u32)>,
    ) -> Result<Armed, RejectReason> {
        self.arm_with(item_id, lib, out_fps, None)
    }

    /// Arm, optionally with data that overrides the Item's own.
    ///
    /// The override is what a live `Update` binds: same Item, same template,
    /// new values. Everything else about arming is identical, which is the
    /// point — a rebind is an arm, and it costs what an arm costs.
    /// Arm against a destination's own template revision.
    ///
    /// The same data rendered by a different template is the whole of ADR 0018:
    /// a narrow strip over the key feed and full-bleed type on the wall are two
    /// templates and one Item.
    fn arm_at(
        &self,
        item_id: &str,
        lib: &dyn Library,
        out_fps: Option<(u32, u32)>,
        template_rev: &str,
    ) -> Result<Armed, RejectReason> {
        self.arm_inner(item_id, lib, out_fps, None, Some(template_rev))
    }

    fn arm_with(
        &self,
        item_id: &str,
        lib: &dyn Library,
        out_fps: Option<(u32, u32)>,
        overrides: Option<&BTreeMap<String, DataValue>>,
    ) -> Result<Armed, RejectReason> {
        self.arm_inner(item_id, lib, out_fps, overrides, None)
    }

    fn arm_inner(
        &self,
        item_id: &str,
        lib: &dyn Library,
        out_fps: Option<(u32, u32)>,
        overrides: Option<&BTreeMap<String, DataValue>>,
        rev_override: Option<&str>,
    ) -> Result<Armed, RejectReason> {
        self.arm_inner_at(item_id, lib, out_fps, overrides, rev_override, self.now_ms)
    }

    #[allow(clippy::too_many_arguments)]
    fn arm_inner_at(
        &self,
        item_id: &str,
        lib: &dyn Library,
        out_fps: Option<(u32, u32)>,
        overrides: Option<&BTreeMap<String, DataValue>>,
        rev_override: Option<&str>,
        taken_at_ms: i64,
    ) -> Result<Armed, RejectReason> {
        let (label, item_rev, data) = lib.item(item_id).ok_or(RejectReason::NoSuchItem)?;
        let template_rev = rev_override.unwrap_or(item_rev);
        let doc = lib.template(template_rev).ok_or(RejectReason::NotReady)?;

        // Refused HERE only as a backstop. The real refusal happens at
        // assignment time, loudly, so you find out Saturday and not at 10:15
        // Sunday (pmg-link.md §4).
        if doc.min_reader > pmg_proto::SCENE_READER_MAJOR {
            return Err(RejectReason::UnreadableTemplate);
        }

        // The Item's data, then anything a live rebind overrode. Merged rather
        // than replaced: an Update that names one field must not blank the rest.
        //
        // Timers resolve HERE, against this node's own clock (ADR 0022) — the
        // number never travels over the link, so a countdown keeps counting
        // when the server dies. `taken_at` is the engine's current wall time at
        // arm; the occupant's own take instant replaces it on the first tick
        // after it goes on air, which is when `elapsed` starts meaning
        // anything.
        let mut bound: pmg_scene::bind::Data = data
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.as_binding_str_at(self.now_ms, self.tz_offset_ms, taken_at_ms),
                )
            })
            .collect();
        if let Some(over) = overrides {
            for (k, v) in over {
                bound.insert(k.clone(), v.as_binding_str());
            }
        }

        // Field-driven media becomes concrete addresses on a COPY, the same
        // pass the designer runs at its arm — one resolution, two surfaces.
        let mut doc = doc.clone();
        pmg_scene::bind::resolve_media_fields(&mut doc, &bound);
        let doc = &doc;

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

        // Open a decoder per clip. `Decoder::open` spawns and returns — the
        // ~120 ms is ffmpeg producing its first frame, and it elapses on the
        // decoder's own thread while this one gets on with the take. Blocking
        // here to pre-roll would hold the engine lock for a hundred times what
        // an arm is supposed to cost, and stall every other output with it.
        let players = out_fps.and_then(|fps| {
            let set: Vec<(usize, pmg_media::video::VideoPlayer)> = instance
                .videos
                .iter()
                .filter_map(|slot| {
                    let (path, spec) = lib.video(&slot.asset_key)?;
                    match pmg_media::video::Decoder::open(&path, &spec, 0, slot.looping) {
                        Ok(dec) => Some((
                            slot.image,
                            pmg_media::video::VideoPlayer::new(dec, fps, slot.looping),
                        )),
                        Err(e) => {
                            // Named and survivable: the poster frame is already
                            // in the texture, so the graphic appears and holds
                            // its first frame rather than the layer going empty.
                            tracing::warn!(key = %slot.asset_key, error = %e,
                                           "could not start a clip; it will hold its first frame");
                            None
                        }
                    }
                })
                .collect();
            // Audio decoders for the same clips. Opened only where ffprobe
            // says there is an audio stream — spawning an ffmpeg to decode
            // silence would be one process per clip for nothing.
            let audio: Vec<pmg_media::audio::AudioDecoder> = instance
                .videos
                .iter()
                .filter_map(|slot| {
                    let (path, _) = lib.video(&slot.asset_key)?;
                    match pmg_media::audio::probe_audio(&path) {
                        Ok(Some(_)) => {
                            match pmg_media::audio::AudioDecoder::open(&path, fps, 0, slot.looping)
                            {
                                Ok(d) => Some(d),
                                Err(e) => {
                                    // Survivable: the clip plays silent rather
                                    // than not at all, and the reason is named.
                                    tracing::warn!(key = %slot.asset_key, error = %e,
                                                   "clip audio would not start; it will play silent");
                                    None
                                }
                            }
                        }
                        _ => None,
                    }
                })
                .collect();

            (!set.is_empty()).then(|| {
                std::sync::Arc::new(std::sync::Mutex::new(PlayerSet {
                    players: set,
                    audio,
                    start_frame: None,
                }))
            })
        });

        Ok(Armed {
            item_id: item_id.to_string(),
            item_label: label.to_string(),
            template_rev: template_rev.to_string(),
            players,
            in_ms: timeline::find(doc, "In")
                .map(|t| t.duration_ms as f64)
                .unwrap_or(0.0),
            out_ms: timeline::find(doc, "Out")
                .map(|t| t.duration_ms as f64)
                .unwrap_or(0.0),
            instance: std::sync::Arc::new(instance),
            doc: doc.clone(),
            // Collected from the Item's OWN data, never from an override: a
            // rebind hands in formatted strings, and a timer that picked those
            // up would stop being a timer after its first tick.
            timers: data
                .iter()
                .filter_map(|(k, v)| match v {
                    DataValue::Timer(spec) => Some((k.clone(), spec.clone())),
                    _ => None,
                })
                .collect(),
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
            // The dissolving graphic runs on the same clock as everything else.
            if let Some(o) = &mut layer.outgoing {
                o.t_ms += dt;
                if o.t_ms >= o.armed.out_ms {
                    layer.outgoing = None;
                }
            }

            let Some(o) = &mut layer.occupant else {
                continue;
            };
            o.t_ms += dt;

            // A rebind advances on the OUTPUT's clock like everything else, and
            // is dropped the moment it lands so the steady state costs one
            // Option check rather than a second draw pass forever.
            if let Some(tw) = &mut o.tween {
                tw.t_ms += dt;
                if tw.done() {
                    o.tween = None;
                }
            }

            // Named runs advance on the same clock. Completion is where the
            // format's `loop` and `on_complete` take effect — until then a run
            // is just a `t` that grows.
            let mut evict = false;
            let mut chained: Vec<String> = Vec::new();
            for run in &mut o.named {
                if !run.running {
                    continue;
                }
                run.t_ms += dt;
                let Some(tl) = timeline::find(&o.armed.doc, &run.name) else {
                    run.running = false;
                    continue;
                };
                let dur = (tl.duration_ms as f64).max(1.0);
                if run.t_ms < dur {
                    continue;
                }
                if tl.looping {
                    run.t_ms %= dur;
                    continue;
                }
                match &tl.on_complete {
                    // Hold: the run STAYS, asserting its final values — that
                    // is what a designer means by it. It merely stops ticking.
                    pmg_scene::doc::OnComplete::Hold => {
                        run.running = false;
                        run.t_ms = dur;
                    }
                    pmg_scene::doc::OnComplete::Evict => evict = true,
                    pmg_scene::doc::OnComplete::Goto(next) => {
                        run.running = false;
                        run.t_ms = dur;
                        chained.push(next.clone());
                    }
                }
            }
            for next in chained {
                match next.as_str() {
                    "In" => {
                        o.phase = Phase::In;
                        o.t_ms = 0.0;
                    }
                    "Out" => {
                        o.phase = Phase::Out;
                        o.t_ms = 0.0;
                        o.named.clear();
                    }
                    _ if timeline::find(&o.armed.doc, &next).is_some() => {
                        match o.named.iter_mut().find(|r| r.name == next) {
                            Some(r) => {
                                r.t_ms = 0.0;
                                r.running = true;
                            }
                            None => o.named.push(NamedRun {
                                name: next,
                                t_ms: 0.0,
                                running: true,
                            }),
                        }
                    }
                    // A Goto naming a timeline the document does not have: the
                    // compiler already warned at publish; the chain just ends.
                    _ => {}
                }
            }

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
                // A named run finished with `on_complete: evict` — the graphic
                // authored its own exit, an Out by another name.
                _ if evict => {
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
                    sync: o.sync.clone(),
                    fps: o.fps() as f32,
                    late_frames: o.late_frames,
                    preview_port: o.preview_port,
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
                                // Running only — a finished-and-holding run is
                                // a look, not motion, and this field exists so
                                // the operator's button can show the press
                                // took.
                                named: o
                                    .named
                                    .iter()
                                    .filter(|r| r.running)
                                    .map(|r| r.name.clone())
                                    .collect(),
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
///
/// The base phase evaluates first, then every named run in start order — so
/// the last writer per target wins, which is the format's whole blending rule
/// in v1 (`docs/specs/scene-format.md` §6.3).
pub fn frame_values(o: &Occupant) -> std::collections::BTreeMap<String, f32> {
    let (name, t) = match o.phase {
        Phase::In => ("In", o.t_ms),
        // Hold is the In timeline's FINAL state, held.
        Phase::Hold => ("In", o.armed.in_ms),
        Phase::Out => ("Out", o.t_ms),
    };
    let mut values = timeline::find(&o.armed.doc, name)
        .map(|tl| timeline::evaluate(tl, t as f32))
        .unwrap_or_default();
    for run in &o.named {
        if let Some(tl) = timeline::find(&o.armed.doc, &run.name) {
            values.extend(timeline::evaluate(tl, run.t_ms as f32));
        }
    }
    values
}
