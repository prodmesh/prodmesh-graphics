//! Layer compositing: the bottom layer clears, everything above it draws over.
//!
//! This exists because the renderer used to clear unconditionally. With one
//! layer that is invisible; with two, the top layer silently erased the one
//! beneath it — a Bug wiping the Lower Third under it, with no error anywhere.
//! The state report said both were live, because they were. Only the pixels
//! disagreed.

use pmg_render::scene::{GlyphSource, RunGlyph, SceneInstance};
use pmg_render::{Load, SceneRenderer};
use pmg_scene::doc::{FontSpec, SceneDoc};

const W: u32 = 256;
const H: u32 = 256;

/// No text in these scenes; boxes are enough to see who covered whom.
struct NoGlyphs;
impl GlyphSource for NoGlyphs {
    fn run(
        &mut self,
        _text: &str,
        _font: &FontSpec,
        _max_width: Option<f32>,
        _max_lines: usize,
        _align: pmg_scene::doc::Align,
    ) -> Option<pmg_render::scene::Run> {
        None
    }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    }))
    .ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("pmg-compositing-test"),
        required_features: wgpu::Features::empty(),
        required_limits:
            wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
        memory_hints: Default::default(),
        experimental_features: Default::default(),
        trace: Default::default(),
    }))
    .ok()?;
    Some(Gpu { device, queue })
}

/// A scene with one opaque bar 64 px wide and full height, offset from the left
/// by `x` using the root's padding.
///
/// Built from the JSON mirror rather than by hand so it exercises the same
/// deserializer a real template goes through.
fn bar(x: f32, colour: [f32; 4]) -> SceneDoc {
    let json = format!(
        r#"{{
          "format": "pmg.scene",
          "format_version": {{ "major": 1, "minor": 0 }},
          "min_reader": 1,
          "id": "01J8ZKCOMPOSITINGTEST001",
          "name": "bar",
          "canvas": {{ "width": {W}, "height": {H} }},
          "fonts": [],
          "fields": [],
          "assets": [],
          "timelines": [],
          "root": {{
            "id": "root",
            "kind": "box",
            "layout": {{
              "direction": "row",
              "justify": "start",
              "align": "start",
              "width": {{ "Px": {W}.0 }},
              "height": {{ "Px": {H}.0 }},
              "padding": {{ "top": 0.0, "right": 0.0, "bottom": 0.0, "left": {x:?} }}
            }},
            "children": [{{
              "id": "bar",
              "kind": "box",
              "layout": {{ "width": {{ "Px": 64.0 }}, "height": {{ "Px": {H}.0 }} }},
              "style": {{ "fill": {{ "rgba": {colour:?} }}, "opacity": 1.0 }}
            }}]
          }}
        }}"#
    );
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("fixture scene: {e}\n{json}"))
}

fn instance(doc: &SceneDoc) -> SceneInstance {
    let solved = pmg_layout::solve(
        doc,
        &Default::default(),
        &pmg_layout::StubMeasure { advance_ratio: 0.5 },
    )
    .expect("layout solves");
    SceneInstance::build(doc, &solved, &mut NoGlyphs)
}

fn read_back(g: &Gpu, tex: &wgpu::Texture) -> Vec<u8> {
    let bpr = W * 4;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bpr * H) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    g.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    rx.recv().unwrap().unwrap();
    let out = slice.get_mapped_range().unwrap().to_vec();
    buf.unmap();
    out
}

/// Alpha at the middle row, at x.
fn alpha_at(px: &[u8], x: u32) -> u8 {
    px[(((H / 2) * W + x) * 4 + 3) as usize]
}

#[test]
fn a_second_layer_draws_over_the_first_without_erasing_it() {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // Two bars that do not overlap: one at x=16, one at x=160.
    let lower = bar(16.0, [1.0, 0.0, 0.0, 1.0]);
    let upper = bar(160.0, [0.0, 0.0, 1.0, 1.0]);
    let (li, ui) = (instance(&lower), instance(&upper));

    // Composites in linear and encodes on readback, the same as the node.
    let fb = pmg_render::draw::LinearFrame::new(&g.device, W, H);
    let view = fb.view().clone();

    let lr = SceneRenderer::new(&g.device, &g.queue, &li);
    let ur = SceneRenderer::new(&g.device, &g.queue, &ui);

    // Bottom layer clears; the one above composites over it.
    lr.render_with(&g.device, &g.queue, &li, &[], &view, (W, H), Load::Clear);
    ur.render_with(&g.device, &g.queue, &ui, &[], &view, (W, H), Load::Over);

    fb.encode(&g.device, &g.queue);
    let px = read_back(&g, fb.output());
    assert_eq!(alpha_at(&px, 48), 255, "the LOWER layer's bar must survive");
    assert_eq!(
        alpha_at(&px, 192),
        255,
        "the upper layer's bar must be drawn"
    );
    assert_eq!(
        alpha_at(&px, 112),
        0,
        "the gap between them stays transparent"
    );
}

#[test]
fn drawing_both_with_clear_loses_the_lower_layer() {
    // The bug, pinned as a fact rather than a memory: this is what the node did
    // before Load existed, and it is why `render` alone is documented as being
    // correct only for a single-layer output.
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };
    let lower = bar(16.0, [1.0, 0.0, 0.0, 1.0]);
    let upper = bar(160.0, [0.0, 0.0, 1.0, 1.0]);
    let (li, ui) = (instance(&lower), instance(&upper));

    // Composites in linear and encodes on readback, the same as the node.
    let fb = pmg_render::draw::LinearFrame::new(&g.device, W, H);
    let view = fb.view().clone();
    SceneRenderer::new(&g.device, &g.queue, &li).render(
        &g.device,
        &g.queue,
        &li,
        &[],
        &view,
        (W, H),
    );
    SceneRenderer::new(&g.device, &g.queue, &ui).render(
        &g.device,
        &g.queue,
        &ui,
        &[],
        &view,
        (W, H),
    );

    fb.encode(&g.device, &g.queue);
    let px = read_back(&g, fb.output());
    assert_eq!(
        alpha_at(&px, 48),
        0,
        "the lower layer is gone — this is the bug"
    );
    assert_eq!(alpha_at(&px, 192), 255);
}

#[test]
fn a_clipped_node_survives_rendering_at_output_scale() {
    // The on-air bug this pins: a 720p design on a 1080p output scales its
    // GEOMETRY implicitly (the vertex shader divides by the canvas), but the
    // scissor is set in pixels. Left in design units, a clipped band in the
    // lower half of the design drew entirely outside its own scissor and
    // vanished from the output — while the designer, rendering 1:1, showed it
    // perfectly. Same renderer, two answers: the exact disagreement ADR 0001
    // exists to prevent.
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // A W×H design whose bar sits at x=160 with CLIP on the bar's parent.
    let mut doc = bar(160.0, [1.0, 0.0, 0.0, 1.0]);
    doc.root.children[0].clip = true;
    let inst = instance(&doc);

    // Rendered onto a target TWICE the design's size.
    let (tw, th) = (W * 2, H * 2);
    let fb = pmg_render::draw::LinearFrame::new(&g.device, tw, th);
    let view = fb.view().clone();
    SceneRenderer::new(&g.device, &g.queue, &inst).render(
        &g.device,
        &g.queue,
        &inst,
        &[],
        &view,
        (tw, th),
    );
    fb.encode(&g.device, &g.queue);

    // Read the double-size target directly.
    let bpr = tw * 4;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (bpr * th) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: fb.output(),
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(th),
            },
        },
        wgpu::Extent3d {
            width: tw,
            height: th,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit(Some(enc.finish()));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    g.device.poll(wgpu::PollType::wait_indefinitely()).ok();
    rx.recv().unwrap().unwrap();
    let px = slice.get_mapped_range().unwrap().to_vec();
    buf.unmap();

    let alpha = |x: u32, y: u32| px[((y * tw + x) * 4 + 3) as usize];
    // The bar (design x 160..224) lands at pixels 320..448 on the 2× target.
    assert_eq!(
        alpha(384, th / 2),
        255,
        "the clipped bar must survive at output scale"
    );
    // And the clip still CLIPS: just past the bar's scaled right edge is
    // transparent, so the fix did not simply stop scissoring.
    assert_eq!(alpha(452, th / 2), 0, "outside the scaled clip stays clear");
    assert_eq!(alpha(316, th / 2), 0, "before the scaled clip stays clear");
}

#[test]
fn strokes_and_shadows_draw_where_the_format_says() {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // An UNFILLED box with a white stroke and an offset black shadow. The
    // outline must draw (an unfilled box used to be skipped entirely), the
    // middle must stay transparent, and the shadow must appear past the
    // bottom-right edge without leaking past the top-left.
    let mut doc = bar(96.0, [1.0, 0.0, 0.0, 1.0]);
    let b = &mut doc.root.children[0];
    b.style.fill = None;
    b.style.stroke = Some(pmg_scene::doc::Stroke {
        color: pmg_scene::doc::Rgba {
            rgba: [1.0, 1.0, 1.0, 1.0],
            token: None,
        },
        width: 4.0,
    });
    b.style.shadow = Some(pmg_scene::doc::Shadow {
        color: pmg_scene::doc::Rgba {
            rgba: [0.0, 0.0, 0.0, 1.0],
            token: None,
        },
        dx: 12.0,
        dy: 0.0,
        blur: 6.0,
    });
    let inst = instance(&doc);

    let fb = pmg_render::draw::LinearFrame::new(&g.device, W, H);
    let view = fb.view().clone();
    SceneRenderer::new(&g.device, &g.queue, &inst).render(
        &g.device,
        &g.queue,
        &inst,
        &[],
        &view,
        (W, H),
    );
    fb.encode(&g.device, &g.queue);
    let px = read_back(&g, fb.output());

    // The bar spans x 96..160, full height. Mid-height row:
    let a = |x: u32| alpha_at(&px, x);
    assert!(a(98) > 200, "left stroke band draws (got {})", a(98));
    assert_eq!(a(128), 0, "the unfilled middle stays TRANSPARENT");
    assert!(a(158) > 200, "right stroke band draws (got {})", a(158));
    // The shadow: offset +12 with blur 6 reaches past the right edge.
    assert!(
        a(166) > 60,
        "the shadow shows past the right edge (got {})",
        a(166)
    );
    assert_eq!(
        a(80),
        0,
        "nothing leaks past the left edge opposite the offset"
    );
}

/// One 32x32 solid-square "glyph", pen at the origin, baseline 32 below the
/// box top. Effects need ink, not typography — a square shows exactly where
/// the ring and the shadow land.
struct SquareGlyph;
impl GlyphSource for SquareGlyph {
    fn run(
        &mut self,
        _text: &str,
        _font: &FontSpec,
        _max_width: Option<f32>,
        _max_lines: usize,
        _align: pmg_scene::doc::Align,
    ) -> Option<pmg_render::scene::Run> {
        Some(pmg_render::scene::Run {
            glyphs: vec![RunGlyph {
                x: 0.0,
                y: 0.0,
                left: 0,
                top: 32,
                width: 32,
                height: 32,
                coverage: vec![255u8; 32 * 32],
            }],
            ascent: 32.0,
            block_h: 32.0,
        })
    }
}

#[test]
fn text_effects_ring_the_glyph_and_shadow_the_outlined_shape() {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // A white square glyph at (64,64)..(96,96) with a red outline 3 px OUT
    // from the edge and a blue shadow offset +14 in x, softness 6.
    use pmg_scene::doc::*;
    let mut doc = bar(0.0, [0.0, 0.0, 0.0, 0.0]);
    doc.root.layout.padding = Edges {
        top: 64.0,
        right: 0.0,
        bottom: 0.0,
        left: 64.0,
    };
    doc.root.children = vec![Node {
        id: "word".into(),
        display_name: None,
        kind: NodeKind::Text,
        layout: Layout {
            width: Dimension::Px(64.0),
            height: Dimension::Px(64.0),
            ..Default::default()
        },
        transform: Transform2D::default(),
        style: Style {
            fill: Some(Rgba {
                rgba: [1.0, 1.0, 1.0, 1.0],
                token: None,
            }),
            stroke: Some(Stroke {
                color: Rgba {
                    rgba: [1.0, 0.0, 0.0, 1.0],
                    token: None,
                },
                width: 3.0,
            }),
            shadow: Some(Shadow {
                color: Rgba {
                    rgba: [0.0, 0.0, 1.0, 1.0],
                    token: None,
                },
                dx: 14.0,
                dy: 0.0,
                blur: 6.0,
            }),
            ..Default::default()
        },
        clip: false,
        visible_if: None,
        content: Some(Content::Text {
            binding: Binding {
                expr: "X".into(),
                transform: vec![],
            },
            font: FontSpec {
                family: "Test".into(),
                weight: 400,
                size: 32.0,
                features: vec![],
                tracking: 0.0,
                line_height: 1.0,
                token: None,
            },
            valign: Align::default(),
            align: Align::Start,
            max_lines: None,
        }),
        children: vec![],
        unknown: Default::default(),
    }];

    let solved = pmg_layout::solve(
        &doc,
        &Default::default(),
        &pmg_layout::StubMeasure { advance_ratio: 0.5 },
    )
    .expect("layout solves");
    let inst = SceneInstance::build(&doc, &solved, &mut SquareGlyph);

    let fb = pmg_render::draw::LinearFrame::new(&g.device, W, H);
    let view = fb.view().clone();
    SceneRenderer::new(&g.device, &g.queue, &inst).render(
        &g.device,
        &g.queue,
        &inst,
        &[],
        &view,
        (W, H),
    );
    fb.encode(&g.device, &g.queue);
    let px = read_back(&g, fb.output());

    let at = |x: u32, y: u32| {
        let i = ((y * W + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };

    // Dead centre: the white fill, on top of everything.
    let c = at(80, 80);
    assert!(
        c[0] > 220 && c[1] > 220 && c[2] > 220,
        "fill is white, got {c:?}"
    );
    // Just outside the glyph edge: the ring, red and solid.
    let r = at(97, 80);
    assert!(
        r[0] > 180 && r[1] < 90 && r[3] > 150,
        "ring is red, got {r:?}"
    );
    // Past the ring, inside the offset silhouette: the shadow, blue.
    let s = at(108, 80);
    assert!(s[2] > 100 && s[0] < 90, "shadow is blue, got {s:?}");
    // Opposite the offset nothing leaks left of the ring.
    let l = at(56, 80);
    assert_eq!(l[3], 0, "no ink left of the ring, got {l:?}");
    // Far away stays empty.
    assert_eq!(at(200, 200)[3], 0);
}

#[test]
fn a_gradient_walks_its_stops_across_the_rect() {
    let Some(g) = gpu() else {
        eprintln!("no GPU adapter; skipping");
        return;
    };

    // The bar spans x 96..160. A 90° (left-to-right) gradient, red to blue,
    // with the flat fill kept as the first stop — the 1.0 reader's fallback.
    let mut doc = bar(96.0, [1.0, 0.0, 0.0, 1.0]);
    doc.root.children[0].style.gradient = Some(pmg_scene::doc::Gradient {
        angle_deg: 90.0,
        stops: vec![
            pmg_scene::doc::GradientStop {
                color: pmg_scene::doc::Rgba {
                    rgba: [1.0, 0.0, 0.0, 1.0],
                    token: None,
                },
                at: 0.0,
            },
            pmg_scene::doc::GradientStop {
                color: pmg_scene::doc::Rgba {
                    rgba: [0.0, 0.0, 1.0, 1.0],
                    token: None,
                },
                at: 1.0,
            },
        ],
    });
    let inst = instance(&doc);

    let fb = pmg_render::draw::LinearFrame::new(&g.device, W, H);
    let view = fb.view().clone();
    SceneRenderer::new(&g.device, &g.queue, &inst).render(
        &g.device,
        &g.queue,
        &inst,
        &[],
        &view,
        (W, H),
    );
    fb.encode(&g.device, &g.queue);
    let px = read_back(&g, fb.output());

    let at = |x: u32| {
        let i = (((H / 2) * W + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    let left = at(98);
    let mid = at(128);
    let right = at(158);
    assert!(
        left[0] > 200 && left[2] < 60,
        "left end is red, got {left:?}"
    );
    assert!(
        right[2] > 200 && right[0] < 60,
        "right end is blue, got {right:?}"
    );
    assert!(
        mid[0] < left[0] && mid[0] > right[0] && mid[2] > left[2] && mid[2] < right[2],
        "the middle is on the way from red to blue, got {mid:?}"
    );
}

/// Vertical alignment moves the whole block within its box (format 1.5).
///
/// Tested by where the INK lands rather than by reading the document back: the
/// offset is applied at placement, and a test that only checked the field would
/// have passed just as happily with the renderer ignoring it.
#[test]
fn vertical_alignment_moves_the_ink_down_its_box() {
    use pmg_scene::doc::*;

    /// The node's solved rect is 64 wide and 128 tall, holding one 32 px line.
    fn scene(valign: Align) -> pmg_render::scene::SceneInstance {
        let mut doc = bar(0.0, [0.0, 0.0, 0.0, 0.0]);
        doc.root.layout.padding = Edges::default();
        doc.root.children = vec![Node {
            id: "word".into(),
            display_name: None,
            kind: NodeKind::Text,
            layout: Layout {
                width: Dimension::Px(64.0),
                height: Dimension::Px(128.0),
                ..Default::default()
            },
            transform: Transform2D::default(),
            style: Style {
                fill: Some(Rgba {
                    rgba: [1.0, 1.0, 1.0, 1.0],
                    token: None,
                }),
                ..Default::default()
            },
            clip: false,
            visible_if: None,
            content: Some(Content::Text {
                binding: Binding {
                    expr: "X".into(),
                    transform: vec![],
                },
                font: FontSpec {
                    family: "Test".into(),
                    weight: 400,
                    size: 32.0,
                    features: vec![],
                    tracking: 0.0,
                    line_height: 1.0,
                    token: None,
                },
                valign,
                align: Align::Start,
                max_lines: None,
            }),
            children: vec![],
            unknown: Default::default(),
        }];
        let solved = pmg_layout::solve(
            &doc,
            &Default::default(),
            &pmg_layout::StubMeasure { advance_ratio: 0.5 },
        )
        .expect("layout solves");
        pmg_render::scene::SceneInstance::build(&doc, &solved, &mut SquareGlyph)
    }

    /// Where the one glyph's ink lands, in design space.
    fn ink_top(inst: &pmg_render::scene::SceneInstance) -> f32 {
        inst.nodes
            .iter()
            .find_map(|n| match &n.content {
                pmg_render::scene::RenderContent::Text(ink) => ink.fill.first().map(|g| g.y),
                _ => None,
            })
            .expect("one placed glyph")
    }

    let top = ink_top(&scene(Align::Start));
    let middle = ink_top(&scene(Align::Center));
    let bottom = ink_top(&scene(Align::End));

    // A 32 px block in a 128 px box: 96 px of room. Middle is half of it,
    // bottom is all of it.
    assert_eq!(
        middle - top,
        48.0,
        "middle should sit half the free space down"
    );
    assert_eq!(
        bottom - top,
        96.0,
        "bottom should sit at the foot of the box"
    );

    // And the default — what an absent field deserialises to — is the top,
    // which is what every document before format 1.5 renders.
    assert_eq!(ink_top(&scene(Align::default())), top);
}
