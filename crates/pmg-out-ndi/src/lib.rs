//! NDI output backend. **The M1 output.**
//!
//! M1 outputs NDI rather than SDI deliberately: M1 already requires the scene
//! format, layout, text, renderer, WASM designer, server, protocol, and CAS to
//! work for the first time, and adding hardware frame-timing risk on top of
//! that is what turns a slice into a quarter. NDI is pure software, carries
//! alpha natively, and the demo an operator sees is identical. DeckLink lands
//! in M2 on a renderer that already works.
//!
//! NDI wants **straight** alpha; everything internal is premultiplied, so the
//! conversion happens here, at the boundary (`docs/specs/color-and-key.md` §5).
//!
//! The SDK is loaded at runtime rather than linked, so redistribution stays a
//! deployment question (ADR 0003 §Seam).
//!
//! # M0 status
//!
//! Types only.

pub fn selftest() -> Result<(), String> {
    Ok(())
}
