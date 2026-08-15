//! Font database, shaping, line breaking, glyph rasterization, and atlas.
//!
//! In the **pure set**: fonts arrive as **bytes**, never from a system font
//! database and never via CSS. That is what makes WYSIWYG survive across
//! machines (ADR 0005 §7) and it is the single most important rule in this
//! crate — using a CSS font in the browser build would silently break parity in
//! a way spike S1 is designed to catch.
//!
//! Intended stack: `parley` + `swash` + `fontique`, with `harfrust`/`rustybuzz`
//! underneath. See risk R3 and spike S2 for why, and for what will actually be
//! hard.
//!
//! # The two things that will make it look amateur
//!
//! - **Integer-snapped glyph positions.** Broadcast CG slides text
//!   horizontally, and snapped glyphs visibly shudder. Cache at fractional
//!   offsets (⅛ px phases) or use SDFs.
//! - **Incorrectly premultiplied AA edges.** Grayscale antialiasing only —
//!   subpixel-RGB is meaningless over an alpha key — and the edge must be
//!   premultiplied correctly or it fringes over live video. This is *the*
//!   classic broadcast CG bug.
//!
//! # M0 status
//!
//! Types only.

#![forbid(unsafe_code)]

/// Number of subpixel phases a glyph is cached at.
///
/// ⅛ px. Fewer and animated text shudders; more and the atlas churns. Spike S2
/// establishes whether this is the right number on real hardware.
pub const SUBPIXEL_PHASES: u32 = 8;

/// A font, identified by content hash and carried as bytes.
#[derive(Debug, Clone)]
pub struct FontFace {
    pub family: String,
    pub weight: u16,
    pub hash: String,
}

pub fn selftest() -> Result<(), String> {
    Ok(())
}
