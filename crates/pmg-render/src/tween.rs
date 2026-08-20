//! Morphing between two solved layouts.
//!
//! # The feature this exists for
//!
//! A lower third carrying a Bible verse, on air, moving to the next section of
//! the verse: the text cross-fades and **the box grows or shrinks to fit it**.
//! `docs/reference/canyon-hills-service-rundown.md` marks that as the one thing
//! in an hour of service that the current tooling cannot do — *"ProPresenter
//! doesn't support it because it doesn't support responsive design"* — and it is
//! what this project's flexbox solver was built for.
//!
//! # Why this is not a relayout per frame
//!
//! The arm/take split says layout happens once, off air, because regenerating
//! per frame costs 17.55 ms against a 16.68 ms budget (`INTEGRATION-NOTES.md`).
//! That rule stands and this does not break it.
//!
//! **Both ends are solved when the data is rebound** — 0.3–2 ms each, the same
//! cost as an arm, paid once — and every frame after that interpolates between
//! two finished answers. A frame still does no layout. It does slightly more
//! arithmetic than a transform, and arithmetic on a handful of rectangles is not
//! what the 17.55 ms was made of.
//!
//! # Why it cannot be a transform
//!
//! Scaling a band with `transform.scale` is the obvious cheap trick and it is
//! wrong in three visible ways: the corner radii stretch into ellipses, the text
//! inside stretches with the box, and the stroke weight changes. A band that
//! grows must keep its corners, its type size, and its padding. That is a
//! different *layout*, not a scaled copy of the old one — which is exactly why
//! `layout.*` and `transform.*` are separated in the scene format (§3.2).
//!
//! # Matching nodes across the two instances
//!
//! By `id`. The scene format guarantees node ids are **stable and never
//! renumbered** precisely so that things addressed across time keep working
//! (§3.1, the same guarantee timeline tracks rely on). A node in one instance and
//! not the other is one that `visible_if` added or removed, and it fades rather
//! than morphing from nothing.

use crate::scene::{FrameNode, RenderContent, SceneInstance};
use pmg_layout::Rect;
use std::collections::BTreeMap;

/// A rebind in progress: where it came from, where it is going, how far along.
#[derive(Debug, Clone, Copy)]
pub struct Tween {
    /// 0 at the old state, 1 at the new one. Already eased by the caller.
    pub t: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect {
        x: lerp(a.x, b.x, t),
        y: lerp(a.y, b.y, t),
        w: lerp(a.w, b.w, t),
        h: lerp(a.h, b.h, t),
    }
}

/// Does this node draw content that has to cross-fade rather than morph?
///
/// A box morphs: it is the same box at a new size. Text and images do not —
/// "In the beginning God created" and "the heavens and the earth" have no
/// correspondence to interpolate, so they dissolve.
fn is_content(c: &RenderContent) -> bool {
    matches!(c, RenderContent::Text(_) | RenderContent::Image(_))
}

/// The two draw lists for one frame of a tween.
///
/// Drawn in this order: `to` first with the caller's load, then `from` composited
/// over it. Geometry comes **only** from the `to` pass, with rects interpolated,
/// so a translucent band is blended once rather than twice — drawing both boxes
/// at partial opacity would darken the overlap and pulse through the tween.
pub struct TweenFrames {
    /// The new instance: everything, with morphed rects, content fading in.
    pub to: Vec<FrameNode>,
    /// The old instance: content only, at its own rects, fading out. Every
    /// non-content node is at zero opacity, so the box is not drawn twice.
    pub from: Vec<FrameNode>,
}

/// Build both draw lists for a tween at `t`.
///
/// `values` are the timeline values for the *new* instance, so an In or Out
/// animation continues to run while a rebind morphs underneath it.
pub fn tween_frames(
    from: &SceneInstance,
    to: &SceneInstance,
    t: f32,
    values: &BTreeMap<String, f32>,
) -> TweenFrames {
    let t = t.clamp(0.0, 1.0);

    // Where each node was in the old layout, by id.
    let was: BTreeMap<&str, Rect> = from.nodes.iter().map(|n| (n.id.as_str(), n.rect)).collect();

    let mut to_frames = crate::scene::apply_tracks(to, values);
    for (f, n) in to_frames.iter_mut().zip(to.nodes.iter()) {
        // A node the old layout also had morphs from where it was. One it did
        // not have is new — `visible_if` revealed it — so it stays where layout
        // put it and fades in with the rest of the content.
        if let Some(old) = was.get(n.id.as_str()) {
            f.rect = Some(lerp_rect(*old, n.rect, t));
        }
        if is_content(&n.content) {
            f.opacity *= t;
        }
    }

    // Where each node is GOING, so the outgoing pass can be clipped by the same
    // morphing geometry the incoming one is.
    let going: BTreeMap<&str, Rect> = to.nodes.iter().map(|n| (n.id.as_str(), n.rect)).collect();

    // The outgoing content. Its GLYPHS do not move — they are placed in
    // absolute design space and there is nothing to interpolate them to — but
    // the box that CLIPS them must morph with everything else.
    //
    // Getting this wrong is visible and was: with the old rects left in place,
    // a box shrinking to fit shorter text kept clipping the outgoing line to
    // its original width, so the old text hung outside the new box for the
    // length of the tween and then vanished when the pass stopped. Seen on SDI,
    // not in any capture — the frame harness never exercised a shrink.
    let from_frames = crate::scene::apply_tracks(from, &BTreeMap::new())
        .into_iter()
        .zip(from.nodes.iter())
        .map(|(mut f, n)| {
            if let Some(new) = going.get(n.id.as_str()) {
                f.rect = Some(lerp_rect(n.rect, *new, t));
            }
            if is_content(&n.content) {
                f.opacity *= 1.0 - t;
            } else {
                // Drawn by the `to` pass. Zero here rather than skipped, so the
                // two lists stay index-aligned with their instances — and a
                // clipping node still contributes its scissor at zero opacity.
                f.opacity = 0.0;
            }
            f
        })
        .collect();

    TweenFrames {
        to: to_frames,
        from: from_frames,
    }
}

/// Ease for a rebind. Smooth at both ends — a box that starts or stops growing
/// abruptly reads as a glitch rather than as motion.
pub fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    // Cubic in-out.
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let f = -2.0 * t + 2.0;
        1.0 - f * f * f / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::RenderNode;
    use pmg_scene::doc::Transform2D;

    fn node(id: &str, rect: Rect, content: RenderContent) -> RenderNode {
        RenderNode {
            id: id.into(),
            parent: None,
            rect,
            base_transform: Transform2D::default(),
            fill: None,
            gradient: None,
            stroke: None,
            shadow: None,
            radius: 0.0,
            opacity: 1.0,
            clip: false,
            content,
        }
    }

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn inst(nodes: Vec<RenderNode>) -> SceneInstance {
        SceneInstance {
            canvas_w: 1920,
            canvas_h: 1080,
            nodes,
            ..Default::default()
        }
    }

    /// The whole feature in one assertion: a band that has to fit taller text
    /// grows to fit it, through the middle rather than jumping at the end.
    #[test]
    fn the_band_grows_to_fit_the_new_text() {
        let a = inst(vec![node(
            "band",
            rect(100.0, 900.0, 600.0, 80.0),
            RenderContent::Box,
        )]);
        let b = inst(vec![node(
            "band",
            rect(100.0, 860.0, 900.0, 120.0),
            RenderContent::Box,
        )]);

        let at = |t: f32| {
            tween_frames(&a, &b, t, &BTreeMap::new()).to[0]
                .rect
                .expect("a node present in both must morph")
        };

        assert_eq!(at(0.0).w, 600.0, "starts at the old width");
        assert_eq!(at(1.0).w, 900.0, "ends at the new one");
        assert_eq!(at(0.5).w, 750.0, "and is genuinely between them partway");
        assert_eq!(at(0.5).h, 100.0);
        // The top edge moves as the box grows downward-anchored; the point is
        // that every edge is interpolated, not just the size.
        assert_eq!(at(0.5).y, 880.0);
    }

    /// Text has no correspondence to interpolate — "In the beginning" and "the
    /// heavens and the earth" are not the same glyphs moved. It dissolves.
    #[test]
    fn text_cross_fades_while_geometry_morphs() {
        let a = inst(vec![
            node("band", rect(0.0, 0.0, 100.0, 50.0), RenderContent::Box),
            node(
                "line",
                rect(0.0, 0.0, 90.0, 20.0),
                RenderContent::Text(Default::default()),
            ),
        ]);
        let b = inst(vec![
            node("band", rect(0.0, 0.0, 200.0, 50.0), RenderContent::Box),
            node(
                "line",
                rect(0.0, 0.0, 190.0, 20.0),
                RenderContent::Text(Default::default()),
            ),
        ]);

        let f = tween_frames(&a, &b, 0.25, &BTreeMap::new());
        assert!((f.to[1].opacity - 0.25).abs() < 1e-6, "new text fading in");
        assert!(
            (f.from[1].opacity - 0.75).abs() < 1e-6,
            "old text fading out"
        );
        // The sum stays at one, so the run of text does not dip in brightness
        // halfway through the dissolve.
        assert!((f.to[1].opacity + f.from[1].opacity - 1.0).abs() < 1e-6);
    }

    /// A translucent band drawn twice at partial opacity darkens where the two
    /// overlap and pulses across the tween. Geometry comes from one pass only.
    #[test]
    fn the_box_is_drawn_once_not_twice() {
        let a = inst(vec![node(
            "band",
            rect(0.0, 0.0, 100.0, 50.0),
            RenderContent::Box,
        )]);
        let b = inst(vec![node(
            "band",
            rect(0.0, 0.0, 200.0, 50.0),
            RenderContent::Box,
        )]);

        for t in [0.0f32, 0.3, 0.5, 0.9, 1.0] {
            let f = tween_frames(&a, &b, t, &BTreeMap::new());
            assert_eq!(
                f.from[0].opacity, 0.0,
                "the old box must not be drawn at t={t}"
            );
            assert!(f.to[0].opacity > 0.0, "the new box carries the geometry");
        }
    }

    /// A node `visible_if` revealed has nowhere to morph from. It appears where
    /// layout put it rather than growing out of the origin.
    #[test]
    fn a_node_the_old_layout_did_not_have_does_not_morph() {
        let a = inst(vec![node(
            "band",
            rect(0.0, 0.0, 100.0, 50.0),
            RenderContent::Box,
        )]);
        let b = inst(vec![
            node("band", rect(0.0, 0.0, 100.0, 50.0), RenderContent::Box),
            node(
                "title",
                rect(10.0, 60.0, 80.0, 20.0),
                RenderContent::Text(Default::default()),
            ),
        ]);

        let f = tween_frames(&a, &b, 0.5, &BTreeMap::new());
        assert_eq!(f.to[1].rect, None, "no old rect to come from");
        assert!((f.to[1].opacity - 0.5).abs() < 1e-6, "it fades in instead");
    }

    /// The bug seen on SDI: a box shrinking to fit shorter text left the
    /// outgoing line hanging outside it, because the outgoing pass was clipped
    /// by the rect it *started* at rather than the one it is passing through.
    #[test]
    fn the_outgoing_pass_is_clipped_by_the_morphing_box_not_the_old_one() {
        let wide = inst(vec![
            node("band", rect(0.0, 0.0, 900.0, 60.0), RenderContent::Box),
            node(
                "line",
                rect(8.0, 8.0, 880.0, 40.0),
                RenderContent::Text(Default::default()),
            ),
        ]);
        let narrow = inst(vec![
            node("band", rect(0.0, 0.0, 300.0, 60.0), RenderContent::Box),
            node(
                "line",
                rect(8.0, 8.0, 280.0, 40.0),
                RenderContent::Text(Default::default()),
            ),
        ]);

        let f = tween_frames(&wide, &narrow, 0.5, &BTreeMap::new());
        let clipper = f.from[0].rect.expect("the outgoing clip box must morph");
        assert_eq!(clipper.w, 600.0, "halfway between 900 and 300");

        // Both passes agree on the geometry at every point, which is what makes
        // one box rather than two.
        assert_eq!(f.to[0].rect.unwrap().w, clipper.w);
    }

    /// Growing is the case that happened to work by accident — the outgoing
    /// text already fits inside a box that is getting bigger — so it is pinned
    /// here to stop a later fix for shrinking from breaking it.
    #[test]
    fn growing_and_shrinking_both_clip_to_the_same_rect() {
        let small = inst(vec![node(
            "band",
            rect(0.0, 0.0, 100.0, 40.0),
            RenderContent::Box,
        )]);
        let big = inst(vec![node(
            "band",
            rect(0.0, 0.0, 500.0, 40.0),
            RenderContent::Box,
        )]);

        for (a, b, want) in [(&small, &big, 300.0), (&big, &small, 300.0)] {
            let f = tween_frames(a, b, 0.5, &BTreeMap::new());
            assert_eq!(f.from[0].rect.unwrap().w, want);
            assert_eq!(f.to[0].rect.unwrap().w, want);
        }
    }

    #[test]
    fn t_is_clamped_so_a_late_frame_cannot_overshoot() {
        let a = inst(vec![node(
            "b",
            rect(0.0, 0.0, 100.0, 10.0),
            RenderContent::Box,
        )]);
        let b = inst(vec![node(
            "b",
            rect(0.0, 0.0, 200.0, 10.0),
            RenderContent::Box,
        )]);
        assert_eq!(
            tween_frames(&a, &b, 2.5, &BTreeMap::new()).to[0]
                .rect
                .unwrap()
                .w,
            200.0
        );
        assert_eq!(
            tween_frames(&a, &b, -1.0, &BTreeMap::new()).to[0]
                .rect
                .unwrap()
                .w,
            100.0
        );
    }

    #[test]
    fn the_ease_is_smooth_at_both_ends_and_symmetric() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        assert!((ease(0.5) - 0.5).abs() < 1e-6);
        // Slow at the ends, fast in the middle — the shape that reads as motion
        // rather than as a jump.
        assert!(ease(0.1) < 0.1, "eases in");
        assert!(ease(0.9) > 0.9, "eases out");
    }
}
