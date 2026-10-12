//! Font loading, shaping, measurement, and glyph rasterization.
//!
//! Spec: `docs/specs/scene-format.md` §3.3 and §7. Risk: [R3](../../docs/RISKS.md),
//! which names this the thing most likely to make the output look amateur.
//!
//! In the **pure set**: no I/O, no platform, no system font database. Fonts
//! arrive as **bytes** and nothing else. That is what makes WYSIWYG survive
//! across machines — a template that named a system font would render
//! differently on the designer's laptop and the booth Mac, silently.
//!
//! # Why glyphs are identical between native and WASM by construction
//!
//! Everything here is **CPU rasterization**. Given the same font bytes, size,
//! glyph and subpixel phase, the coverage bitmap is the same on both targets
//! because it is the same code doing the same arithmetic. The GPU's only
//! contribution is *sampling* that bitmap — which is exactly what spike S1's
//! alpha-atlas figure measured, and why S1 did not need a text stack to be
//! meaningful.
//!
//! # The three hazards from R3, and what is done about each
//!
//! 1. **Subpixel positioning.** Broadcast CG slides text horizontally, and
//!    integer-snapped glyphs visibly shudder. Glyphs are rasterized at
//!    [`SUBPIXEL_PHASES`] fractional offsets and cached per phase.
//! 2. **Antialiasing over a key.** The atlas stores **coverage only** — a single
//!    alpha channel, never premultiplied colour. Premultiplication happens once,
//!    at composite, in the renderer's linear working space (ADR 0012).
//!    Grayscale only; subpixel-RGB AA is meaningless over an alpha key.
//! 3. **Vertical metrics.** See [`Metrics`]. Chosen once, documented, and never
//!    to be changed.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::Arc;

pub mod atlas;

pub use atlas::{GlyphAtlas, GlyphBitmap, GlyphKey};

/// Fractional horizontal positions a glyph is rasterized at: **⅛ px**.
///
/// Fewer and animated text shudders as glyphs snap between whole pixels; more
/// and the atlas churns for no visible gain. Spike S2 is what confirms ⅛ is the
/// right number on real hardware — until then this is a reasoned default, and
/// it is the single most important constant in this crate.
pub const SUBPIXEL_PHASES: u32 = 8;

/// Quantise a horizontal position into a whole pixel plus a subpixel phase.
///
/// Returns `(whole_px, phase)` where `phase` is in `0..SUBPIXEL_PHASES`.
pub fn quantize(x: f32) -> (i32, u8) {
    let scaled = (x * SUBPIXEL_PHASES as f32).round() as i64;
    let whole = scaled.div_euclid(SUBPIXEL_PHASES as i64) as i32;
    let phase = scaled.rem_euclid(SUBPIXEL_PHASES as i64) as u8;
    (whole, phase)
}

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("font bytes could not be parsed")]
    BadFont,
    #[error("no face matches family {family:?} weight {weight}")]
    NoFace { family: String, weight: u16 },
}

/// A loaded face. Owns its bytes so a `swash::FontRef` can be made on demand —
/// `FontRef` borrows, and holding one across the cache would fight the borrow
/// checker for no benefit.
struct Face {
    bytes: Arc<Vec<u8>>,
    offset: u32,
    family: String,
    weight: u16,
    italic: bool,
}

impl Face {
    fn font(&self) -> swash::FontRef<'_> {
        // Offset was validated when the face was added.
        swash::FontRef::from_offset(&self.bytes, self.offset)
            .or_else(|| swash::FontRef::from_index(&self.bytes, 0))
            .expect("validated")
    }
}

/// What one face in a font file says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceInfo {
    /// Index within a collection (`.ttc`/`.otc`); 0 for a single font.
    pub index: usize,
    pub family: String,
    pub weight: u16,
    /// Italic or oblique. Matching treats them alike: a family ships one or
    /// the other, and a template asking for a slant wants whichever it has.
    pub italic: bool,
}

/// Every face in a font file — one for a `.ttf`, several for a collection.
///
/// A collection used to contribute only its first face, so the Bold and Italic
/// inside `Helvetica.ttc` were in the file and unreachable.
pub fn faces_in(bytes: &[u8]) -> Vec<FaceInfo> {
    use swash::StringId;
    let Some(data) = swash::FontDataRef::new(bytes) else {
        return Vec::new();
    };
    (0..data.len())
        .filter_map(|index| {
            let font = data.get(index)?;
            let strings = font.localized_strings();
            let family = strings
                .find_by_id(StringId::TypographicFamily, None)
                .or_else(|| strings.find_by_id(StringId::Family, None))
                .map(|s| s.chars().collect::<String>())
                .filter(|s| !s.trim().is_empty())?;
            let attrs = font.attributes();
            Some(FaceInfo {
                index,
                family,
                weight: attrs.weight().0,
                italic: attrs.style() != swash::Style::Normal,
            })
        })
        .collect()
}

/// What a font file says about itself: the name to offer in a picker, and the
/// weight the designer gets by picking it. The first face of a collection.
///
/// The **typographic** family is preferred over the plain one — "IBM Plex
/// Sans", not "IBM Plex Sans SemiBold" — because that is the axis a family
/// dropdown groups by; the weight carries the rest.
pub fn face_info(bytes: &[u8]) -> Option<(String, u16)> {
    faces_in(bytes)
        .into_iter()
        .next()
        .map(|f| (f.family, f.weight))
}

/// Build a database from raw font files and the fonts a document declares.
///
/// Three rules, in order, and the order is the design:
///
/// 1. **Every face registers under its own name**, weight and slant — each
///    face of a collection, not just the first. A document naming two real
///    families gets two real faces.
/// 2. **A declaration that pins a file by hash gets that file**, under the
///    declared family. The hash is the document's own statement of which bytes
///    it means (`fonts[].asset`), so a family renamed in the document, or one
///    whose file calls itself something else, still resolves to the face the
///    designer chose rather than to a guess.
/// 3. **A declared family nothing answered to gets every face**, at each
///    face's OWN weight and slant — the M1 shim, kept as the fallback so an
///    old document, or one whose font is missing, renders rather than failing
///    on bookkeeping. It used to register them all at the declared weight,
///    where they tied and the first file loaded won, so a missing family lost
///    its bold.
///
/// One implementation, used by the browser designer and the playout node —
/// two copies of this rule would be two ways for the preview and the output
/// to disagree about which font a family means.
pub fn build_font_db(
    faces: &[Vec<u8>],
    declared: &[pmg_scene::doc::FontRef],
) -> Result<FontDb, TextError> {
    let mut db = FontDb::new();
    let files: Vec<(Arc<Vec<u8>>, Vec<FaceInfo>)> = faces
        .iter()
        .map(|b| (Arc::new(b.clone()), faces_in(b)))
        .collect();

    for (bytes, infos) in &files {
        for f in infos {
            db.add_shared(bytes.clone(), f.index, f.family.clone(), f.weight, f.italic)?;
        }
    }

    // Hashed once per file, and only when something is pinned: fonts run to
    // megabytes, and most documents pin nothing the file names don't already say.
    let addresses: Vec<String> = if declared.iter().any(|d| d.asset.is_some()) {
        files
            .iter()
            .map(|(b, _)| pmg_scene::validate::address_of(b))
            .collect()
    } else {
        Vec::new()
    };
    for d in declared {
        let Some(hash) = d.asset.as_deref() else {
            continue;
        };
        for ((bytes, infos), address) in files.iter().zip(&addresses) {
            if address != hash {
                continue;
            }
            for f in infos
                .iter()
                .filter(|f| !f.family.eq_ignore_ascii_case(&d.family))
            {
                db.add_shared(bytes.clone(), f.index, d.family.clone(), f.weight, f.italic)?;
            }
        }
    }

    let mut fell_back: Vec<String> = Vec::new();
    for d in declared {
        if db.has_family(&d.family) || fell_back.iter().any(|f| f.eq_ignore_ascii_case(&d.family)) {
            continue;
        }
        for (bytes, infos) in &files {
            for f in infos {
                db.add_shared(bytes.clone(), f.index, d.family.clone(), f.weight, f.italic)?;
            }
        }
        fell_back.push(d.family.clone());
    }
    Ok(db)
}

/// Faces available to a document, loaded from bytes.
///
/// **There is deliberately no system font enumeration here.** A scene may name
/// a system font only through an explicit fallback chain, and Design flags it
/// as *this will differ between machines* (`docs/specs/scene-format.md` §7).
#[derive(Default)]
pub struct FontDb {
    faces: Vec<Face>,
}

/// Index into a [`FontDb`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u32);

impl FontDb {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a face from font bytes, upright, under the given name and weight.
    pub fn add_face(
        &mut self,
        bytes: Vec<u8>,
        family: impl Into<String>,
        weight: u16,
    ) -> Result<FaceId, TextError> {
        self.add_shared(Arc::new(bytes), 0, family.into(), weight, false)
    }

    /// Load face `index` of a file, stating its slant as well.
    pub fn add_face_styled(
        &mut self,
        bytes: Vec<u8>,
        index: usize,
        family: impl Into<String>,
        weight: u16,
        italic: bool,
    ) -> Result<FaceId, TextError> {
        self.add_shared(Arc::new(bytes), index, family.into(), weight, italic)
    }

    fn add_shared(
        &mut self,
        bytes: Arc<Vec<u8>>,
        index: usize,
        family: String,
        weight: u16,
        italic: bool,
    ) -> Result<FaceId, TextError> {
        let offset = swash::FontRef::from_index(&bytes, index)
            .ok_or(TextError::BadFont)?
            .offset;
        self.faces.push(Face {
            bytes,
            offset,
            family,
            weight,
            italic,
        });
        Ok(FaceId(self.faces.len() as u32 - 1))
    }

    /// The file a face was loaded from — what a family actually resolved to.
    ///
    /// Asked by the question "which font is this text really in?", which is
    /// the one a designer has when a preview and an output disagree.
    pub fn bytes_of(&self, id: FaceId) -> Option<&[u8]> {
        self.faces.get(id.0 as usize).map(|f| f.bytes.as_slice())
    }

    /// Whether any face answers to this family name.
    pub fn has_family(&self, family: &str) -> bool {
        self.faces
            .iter()
            .any(|f| f.family.eq_ignore_ascii_case(family))
    }

    /// The closest UPRIGHT face for a family and weight. See [`Self::select_style`].
    pub fn select(&self, family: &str, weight: u16) -> Option<FaceId> {
        self.select_style(family, weight, false)
    }

    /// Pick the closest face for a family, weight and slant.
    ///
    /// The slant is decided first: with Regular and Italic both at 400, plain
    /// text must get Regular every time, and it used to get whichever file
    /// happened to load first. A family with no face in the asked-for slant
    /// falls back to the other rather than to nothing.
    ///
    /// Weight then follows CSS Fonts §5.2, so a template means the same thing
    /// here as in the browser that designed it: from 400–500, heavier up to
    /// 500 first, then lighter, then heavier; below 400, lighter first; above
    /// 500, heavier first. Nearest-by-distance agreed with that except on
    /// ties, and a tie broken by load order is the kind of answer that changes
    /// when somebody uploads one more file. A missing lower third is worse
    /// than a slightly wrong one, so there is always an answer if the family
    /// has any face at all.
    pub fn select_style(&self, family: &str, weight: u16, italic: bool) -> Option<FaceId> {
        let in_family = || {
            self.faces
                .iter()
                .enumerate()
                .filter(move |(_, f)| f.family.eq_ignore_ascii_case(family))
        };
        let want_slant = in_family().any(|(_, f)| f.italic == italic);
        in_family()
            .filter(|(_, f)| !want_slant || f.italic == italic)
            .min_by_key(|(i, f)| (weight_rank(weight, f.weight), *i))
            .map(|(i, _)| FaceId(i as u32))
    }

    pub fn len(&self) -> usize {
        self.faces.len()
    }
    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    fn face(&self, id: FaceId) -> Option<&Face> {
        self.faces.get(id.0 as usize)
    }
}

/// Vertical metrics for a face at a size, in pixels.
///
/// # The choice that can never be changed
///
/// **OS/2 typographic metrics when the font sets `USE_TYPO_METRICS`
/// (fsSelection bit 7), otherwise hhea.** This is swash's rule, it is the rule
/// browsers use, and adopting it means our text sits where a designer expects
/// rather than two pixels off from every other renderer they have used.
///
/// It reads as conditional, but it cannot drift: **fonts are content-addressed
/// assets**, so for a given font hash the flag — and therefore the metrics — is
/// fixed forever. A template pinned to a font hash gets the same baseline in
/// three years.
///
/// Changing this rule would move every existing template by a couple of pixels,
/// silently. Do not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    pub units_per_em: u16,
}

impl Metrics {
    /// Distance from one baseline to the next, before the document's
    /// `line_height` multiplier is applied.
    pub fn natural_line_height(&self) -> f32 {
        self.ascent + self.descent + self.leading
    }
}

/// One glyph, positioned on the baseline.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionedGlyph {
    pub id: u16,
    /// Horizontal offset from the run origin, in pixels. Fractional on purpose.
    pub x: f32,
    /// Vertical offset from the baseline, in pixels.
    pub y: f32,
    /// Byte offset in the source string of the cluster this glyph came from.
    ///
    /// Carried so a shaped run can be **split into lines after shaping** rather
    /// than shaped word by word. Shaping each word separately would lose the
    /// kerning between the last letter of one word and the space after it, and
    /// would measure differently from the same string drawn unwrapped.
    pub cluster: u32,
}

/// A shaped run of text.
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedRun {
    pub glyphs: Vec<PositionedGlyph>,
    pub width: f32,
    pub metrics: Metrics,
}

/// Shaping, measurement, and rasterization for one document's fonts.
pub struct TextEngine {
    pub db: FontDb,
    atlas: GlyphAtlas,
    /// Cached metrics per (face, quantised size).
    metrics: HashMap<(FaceId, u32), Metrics>,
}

impl TextEngine {
    pub fn new(db: FontDb) -> Self {
        Self {
            db,
            atlas: GlyphAtlas::new(),
            metrics: HashMap::new(),
        }
    }

    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    /// Vertical metrics for a face at a size. See [`Metrics`] for the rule.
    pub fn metrics(&mut self, face: FaceId, size_px: f32) -> Option<Metrics> {
        let key = (face, size_px.to_bits());
        if let Some(m) = self.metrics.get(&key) {
            return Some(*m);
        }
        let f = self.db.face(face)?;
        let font = f.font();
        // scale(ppem), NOT linear_scale(). linear_scale multiplies raw design
        // units by its argument; scale() divides by units_per_em first, which is
        // what turns design units into pixels. Getting this wrong gives an
        // ascent of 85284 "px" at 46 px and is invisible to any test that only
        // checks relationships — both numbers are wrong by the same factor.
        let m = font.metrics(&[]).scale(size_px);
        let out = Metrics {
            ascent: m.ascent,
            descent: m.descent,
            leading: m.leading,
            units_per_em: m.units_per_em,
        };
        self.metrics.insert(key, out);
        Some(out)
    }

    /// Shape a run.
    ///
    /// Features are passed through from the document. **`tnum` matters** —
    /// clocks and scores jitter without tabular figures
    /// (`docs/specs/scene-format.md` §3.3).
    pub fn shape(
        &mut self,
        face: FaceId,
        text: &str,
        size_px: f32,
        features: &[String],
        tracking: f32,
    ) -> Option<ShapedRun> {
        let metrics = self.metrics(face, size_px)?;
        let f = self.db.face(face)?;
        let font = f.font();

        // (&str, u16) converts straight into swash's Setting. Value 1 means
        // "on"; a future format revision that wants alternates would carry the
        // value rather than just the tag.
        let feats: Vec<swash::Setting<u16>> =
            features.iter().map(|t| (t.as_str(), 1u16).into()).collect();

        let mut ctx = swash::shape::ShapeContext::new();
        let mut shaper = ctx
            .builder(font)
            .size(size_px)
            .features(feats.iter().copied())
            .build();
        shaper.add_str(text);

        let mut glyphs = Vec::new();
        let mut pen = 0.0f32;
        shaper.shape_with(|cluster| {
            let source = cluster.source.start;
            for g in cluster.glyphs {
                glyphs.push(PositionedGlyph {
                    id: g.id,
                    x: pen + g.x,
                    y: g.y,
                    cluster: source,
                });
                // Tracking is a fraction of em, applied per glyph advance.
                pen += g.advance + tracking * size_px;
            }
        });

        Some(ShapedRun {
            glyphs,
            width: pen.max(0.0),
            metrics,
        })
    }

    /// Rasterize a glyph at a subpixel phase, caching the result.
    ///
    /// Returns **alpha coverage**, never premultiplied colour — see the module
    /// note on hazard 2.
    pub fn glyph(
        &mut self,
        face: FaceId,
        glyph: u16,
        size_px: f32,
        phase: u8,
    ) -> Option<&GlyphBitmap> {
        let key = GlyphKey {
            face,
            size_bits: size_px.to_bits(),
            glyph,
            phase,
        };
        if !self.atlas.contains(&key) {
            let f = self.db.face(face)?;
            let font = f.font();
            let bmp = atlas::rasterize(font, glyph, size_px, phase)?;
            self.atlas.insert(key, bmp);
        }
        self.atlas.get(&key)
    }
}

/// Real text measurement for the layout solver.
///
/// Replaces `pmg_layout::StubMeasure`. M1b's tests assert *relationships*
/// between rectangles rather than absolute widths precisely so they keep
/// passing when this takes over.
///
/// Wraps the engine in a `RefCell` because measuring populates caches —
/// `TextMeasure::measure` takes `&self` since the solver calls it from a
/// callback, and shaping legitimately mutates.
pub struct Measurer(std::cell::RefCell<TextEngine>);

impl Measurer {
    pub fn new(engine: TextEngine) -> Self {
        Self(std::cell::RefCell::new(engine))
    }
    /// Take the engine back, with its caches warm — the renderer wants the
    /// atlas that measuring just filled.
    pub fn into_engine(self) -> TextEngine {
        self.0.into_inner()
    }
    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, TextEngine> {
        self.0.borrow_mut()
    }
}

/// Where a line may be broken: the byte offset of each word start after a run
/// of whitespace, plus 0.
///
/// **Whitespace only — this is not UAX #14.** No hyphenation, no CJK, no
/// break-after-slash. That is a real limitation and it is the honest shape of
/// what a greedy space-breaker can promise; the Unicode algorithm arrives with
/// parley. For English lyrics and scripture — every line in the reference
/// service — a space is where a line breaks, and it is worth having that now
/// rather than one-line graphics until M5.
fn break_offsets(s: &str) -> Vec<u32> {
    let mut out = vec![0u32];
    let mut prev_space = false;
    for (i, c) in s.char_indices() {
        if prev_space && !c.is_whitespace() {
            out.push(i as u32);
        }
        prev_space = c.is_whitespace();
    }
    out
}

/// How a run may be broken into lines.
///
/// One value rather than two loose arguments because they are always decided
/// together — a width with no cap and a cap with no width are both meaningful,
/// and threading them separately through four call sites invited them to drift
/// apart.
#[derive(Debug, Clone, Copy)]
pub struct Wrap {
    /// The box the solver already decided on. `None` measures unconstrained,
    /// so only a hard `\n` breaks.
    pub max_width: Option<f32>,
    /// Lines to keep. Truncates; it does not ellipsize — the format has an
    /// `ellipsize` binding transform and two mechanisms for one decision is
    /// one too many.
    pub max_lines: usize,
}

impl Default for Wrap {
    fn default() -> Self {
        // Unbounded, because a default that clipped would silently reimpose
        // the single-line behaviour this replaced.
        Self {
            max_width: None,
            max_lines: usize::MAX,
        }
    }
}

impl TextEngine {
    /// Shape into lines, honouring hard breaks and wrapping to a width.
    ///
    /// Two different things, and the first is the one that matters most for a
    /// lyric slide: **a `\n` in the data is a break the author chose.** "We are
    /// here to witness / to share and testify" is a decision about phrasing,
    /// and re-flowing it to the box width would be overriding the person who
    /// wrote the slide.
    ///
    /// Wrapping is greedy and splits an ALREADY-SHAPED run at cluster
    /// boundaries, rather than shaping word by word. That keeps kerning inside
    /// each word identical to the unwrapped string, so a line that fits does
    /// not shift when a later line wraps.
    ///
    /// `max_lines` truncates. It does not ellipsize: the format has an
    /// `ellipsize` binding transform for that, and doing it here as well would
    /// be two mechanisms for one decision.
    pub fn shape_lines(
        &mut self,
        face: FaceId,
        text: &str,
        size_px: f32,
        features: &[String],
        tracking: f32,
        wrap: Wrap,
    ) -> Option<Vec<ShapedRun>> {
        let Wrap {
            max_width,
            max_lines,
        } = wrap;
        let mut lines: Vec<ShapedRun> = Vec::new();
        for segment in text.split('\n') {
            if lines.len() >= max_lines.max(1) {
                break;
            }
            let run = self.shape(face, segment, size_px, features, tracking)?;
            let room = match max_width {
                Some(w) if w > 0.0 => w,
                // No constraint: the hard break is the only break.
                _ => {
                    lines.push(run);
                    continue;
                }
            };
            if run.width <= room || run.glyphs.is_empty() {
                lines.push(run);
                continue;
            }
            wrap_run(&run, segment, room, max_lines.max(1), &mut lines);
        }
        lines.truncate(max_lines.max(1));
        Some(lines)
    }
}

/// Split one shaped run into lines no wider than `room`.
fn wrap_run(run: &ShapedRun, segment: &str, room: f32, max_lines: usize, out: &mut Vec<ShapedRun>) {
    let breaks = break_offsets(segment);
    let mut start_glyph = 0usize;
    let mut line_origin = 0.0f32;

    while start_glyph < run.glyphs.len() && out.len() < max_lines {
        // The first glyph that would overflow this line.
        let over = run.glyphs[start_glyph..]
            .iter()
            .position(|g| g.x - line_origin > room)
            .map(|i| start_glyph + i);

        let Some(over) = over else {
            out.push(slice_run(
                run,
                segment,
                start_glyph,
                run.glyphs.len(),
                line_origin,
            ));
            return;
        };

        // Break at the last opportunity at or before the overflowing cluster.
        // Strictly after this line's own start, or a word longer than the box
        // would break at its own start and loop forever.
        let cut = breaks
            .iter()
            .copied()
            .filter(|b| *b > run.glyphs[start_glyph].cluster && *b <= run.glyphs[over].cluster)
            .next_back();

        let at = |b: u32| run.glyphs.iter().position(|g| g.cluster >= b);
        let end_glyph = match cut {
            Some(b) => at(b).unwrap_or(run.glyphs.len()),
            // No break opportunity inside the overflowing span: this is one
            // word wider than the box. Extend to the END of that word rather
            // than cutting at the overflowing glyph — cutting there chops a
            // long word into one line per glyph, which is what this did first.
            // Overflowing on one line is clipped and visible; a designer can
            // see a word is too long and cannot see one silently shredded.
            None => breaks
                .iter()
                .copied()
                .find(|b| *b > run.glyphs[over].cluster)
                .and_then(at)
                .unwrap_or(run.glyphs.len()),
        };
        let end_glyph = end_glyph.max(start_glyph + 1).min(run.glyphs.len());

        out.push(slice_run(run, segment, start_glyph, end_glyph, line_origin));
        if end_glyph >= run.glyphs.len() {
            return;
        }
        line_origin = run.glyphs[end_glyph].x;
        start_glyph = end_glyph;
    }
}

/// One line's glyphs, re-origined to x = 0.
fn slice_run(run: &ShapedRun, segment: &str, from: usize, to: usize, origin: f32) -> ShapedRun {
    let glyphs: Vec<PositionedGlyph> = run.glyphs[from..to]
        .iter()
        .map(|g| PositionedGlyph {
            x: g.x - origin,
            ..*g
        })
        .collect();

    // Width to the end of the last VISIBLE glyph. A line broken at a space
    // carries that space, and counting its advance would widen the line by a
    // space — which pushes a centred lyric off centre by half a space, on every
    // line, differently. Trailing whitespace is measured as nothing.
    let last_ink = run.glyphs[from..to].iter().rposition(|g| {
        segment[g.cluster as usize..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_whitespace())
    });
    let width = match last_ink {
        None => 0.0,
        Some(i) => {
            let after = from + i + 1;
            if after < run.glyphs.len() {
                run.glyphs[after].x - origin
            } else {
                run.width - origin
            }
        }
    };
    ShapedRun {
        glyphs,
        width: width.max(0.0),
        metrics: run.metrics,
    }
}

impl pmg_layout::TextMeasure for Measurer {
    fn measure(
        &self,
        text: &str,
        font: &pmg_scene::doc::FontSpec,
        max_width: Option<f32>,
        max_lines: usize,
    ) -> (f32, f32) {
        let mut eng = self.0.borrow_mut();
        let Some(face) = eng
            .db
            .select_style(&font.family, font.weight, font.italic())
        else {
            // A missing face measures as nothing rather than panicking. The
            // document is still laid out, and the absent font is a validation
            // concern rather than a render-thread failure.
            return (0.0, 0.0);
        };
        // Measured the same way it is drawn — the SAME call the renderer makes.
        // Measuring one way and drawing another is how a box ends up the wrong
        // size for the words in it, and it is the bug this replaced: measure
        // reported one line while the format declared four, so a lyric slide
        // and a scripture block were laid out for text that would not fit.
        let Some(lines) = eng.shape_lines(
            face,
            text,
            font.size,
            &font.features,
            font.tracking,
            Wrap {
                max_width,
                max_lines,
            },
        ) else {
            return (0.0, 0.0);
        };
        let Some(first) = lines.first() else {
            return (0.0, 0.0);
        };
        let line_h = first.metrics.natural_line_height() * font.line_height;
        let widest = lines.iter().fold(0.0f32, |a, l| a.max(l.width));
        (widest, line_h * lines.len() as f32)
    }
}

pub fn selftest() -> Result<(), String> {
    // Pure arithmetic first — these hold with or without a font on the machine.
    for (x, want_px, want_phase) in [
        (0.0, 0, 0u8),
        (1.0, 1, 0),
        (1.125, 1, 1),
        (1.5, 1, 4),
        (1.875, 1, 7),
        (2.0, 2, 0),
    ] {
        let (px, ph) = quantize(x);
        if px != want_px || ph != want_phase {
            return Err(format!(
                "quantize({x}) = ({px},{ph}), expected ({want_px},{want_phase})"
            ));
        }
    }
    // Negative positions must quantise consistently, not toward zero — text
    // sliding in from the left crosses zero and must not jump there.
    let (px, ph) = quantize(-0.125);
    if px != -1 || ph != 7 {
        return Err(format!("quantize(-0.125) = ({px},{ph}), expected (-1,7)"));
    }
    if SUBPIXEL_PHASES != 8 {
        return Err("SUBPIXEL_PHASES changed — every cached glyph phase depends on it".into());
    }
    Ok(())
}

/// How well `have` serves a request for `want`, lower is better — the CSS
/// Fonts §5.2 order as a sort key: (preference tier, distance).
fn weight_rank(want: u16, have: u16) -> (u8, u16) {
    let d = want.abs_diff(have);
    if d == 0 {
        return (0, 0);
    }
    if (400..=500).contains(&want) {
        if have > want && have <= 500 {
            (1, d)
        } else if have < want {
            (2, d)
        } else {
            (3, d)
        }
    } else if want < 400 {
        if have < want {
            (1, d)
        } else {
            (2, d)
        }
    } else if have > want {
        (1, d)
    } else {
        (2, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selftest_passes() {
        selftest().unwrap();
    }

    #[test]
    fn quantize_round_trips_within_an_eighth() {
        for i in 0..200 {
            let x = i as f32 * 0.05 - 5.0;
            let (px, ph) = quantize(x);
            let back = px as f32 + ph as f32 / SUBPIXEL_PHASES as f32;
            assert!(
                (back - x).abs() <= 0.5 / SUBPIXEL_PHASES as f32 + 1e-6,
                "quantize({x}) -> {back} is further than half a phase away"
            );
        }
    }
}
