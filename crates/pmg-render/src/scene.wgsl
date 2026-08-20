// Scene rendering — boxes and text.
//
// The determinism rules from parity.wgsl apply here unchanged, because the S1
// corpus exists to protect exactly this code:
//
//   1. NO fwidth / dpdx / dpdy. Derivative granularity is implementation-
//      defined, so AA built on it differs between backends BY SPECIFICATION.
//      Pixel scale arrives as a uniform.
//   2. NO MSAA. Sample patterns are unspecified across backends.
//   3. Target is Rgba8Unorm, never Rgba8UnormSrgb — no automatic colour
//      conversion in the pipeline.
//   4. Every sampler field stated on the Rust side; no filtering defaults.
//
// Everything writes PREMULTIPLIED alpha into a linear target. Premultiplication
// happens here, once, and never in the glyph atlas (ADR 0012).

struct Node {
    // Destination rect in design space: x, y, w, h.
    rect        : vec4<f32>,
    // Fill colour, STRAIGHT alpha as authored. Premultiplied on write.
    color       : vec4<f32>,
    // 2x3 affine, row-major: (m00, m01, m10, m11, tx, ty).
    xform       : vec4<f32>,
    xform_t     : vec2<f32>,
    radius      : f32,
    opacity     : f32,
    // Canvas size, so design space can be mapped to clip space without a
    // separate uniform binding.
    canvas      : vec2<f32>,
    // One design-space unit per pixel. Replaces fwidth() — see rule 1.
    px_per_unit : f32,
    _pad        : f32,
    // Sub-rectangle of an image to sample: x, y, w, h, normalised.
    uv          : vec4<f32>,
    // Stroke colour, LINEAR, straight alpha. Alpha 0 means no stroke.
    stroke_color : vec4<f32>,
    // Shadow colour, LINEAR, straight alpha. Alpha 0 means no shadow.
    shadow_color : vec4<f32>,
    // x: stroke width, y/z: shadow dx/dy, w: shadow blur. Design units.
    effect       : vec4<f32>,
};

@group(0) @binding(0) var<uniform> u : Node;

struct VsOut {
    @builtin(position) pos   : vec4<f32>,
    // Position within the node's rect, in design-space pixels.
    @location(0)       local : vec2<f32>,
    @location(1)       uv    : vec2<f32>,
};

// Apply the node's affine transform about its anchor, then map design space to
// clip space. Done in the vertex shader so the fragment shader only ever deals
// in the node's own local space — which is what keeps the SDF math simple and
// exact.
fn to_clip(p : vec2<f32>) -> vec4<f32> {
    let m = mat2x2<f32>(u.xform.x, u.xform.y, u.xform.z, u.xform.w);
    let world = m * p + u.xform_t;
    // Design space is y-down from the top-left; clip space is y-up centred.
    let ndc = vec2<f32>(
         (world.x / u.canvas.x) * 2.0 - 1.0,
        1.0 - (world.y / u.canvas.y) * 2.0,
    );
    return vec4<f32>(ndc, 0.0, 1.0);
}

// ---------------------------------------------------------------------------
// Boxes — an analytically antialiased rounded rectangle.
// ---------------------------------------------------------------------------

// How far the quad extends past the rect, in design units.
//
// The coverage ramp reaches half a pixel OUTSIDE the geometric edge — a pixel
// whose centre sits just past the edge is still partially covered — but the
// rasterizer only shades pixels whose centres the quad reaches. Sizing the
// quad exactly to the rect therefore truncated the outer half of the ramp,
// and the truncation is what spike S2 measured as the 0.21 px stutter: the
// outermost column snapped from unshaded to ~half coverage at the instant a
// sliding edge crossed its pixel centre, a staircase with a 1 px period. One
// unit of outset covers the ramp with margin; the SDF returns zero coverage
// over the surplus, so the ring costs a few fragments and draws nothing.
const AA_OUTSET : f32 = 1.0;

@vertex
fn vs_box(@builtin(vertex_index) i : u32) -> VsOut {
    var corner = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corner[i];
    // A shadow reaches past the rect by its offset plus its softness, and the
    // quad has to cover it or the rasterizer never shades those pixels — the
    // same truncation lesson AA_OUTSET already encodes, at a larger radius.
    // Zero for a shadowless node, so its quad (and pixels) are unchanged.
    let shadow_reach = select(
        0.0,
        max(abs(u.effect.y), abs(u.effect.z)) + u.effect.w + AA_OUTSET,
        u.shadow_color.a > 0.0,
    );
    let outset = AA_OUTSET + shadow_reach;
    let grown = u.rect.zw + vec2<f32>(2.0 * outset, 2.0 * outset);
    let p = u.rect.xy - vec2<f32>(outset, outset) + c * grown;

    var out : VsOut;
    out.pos   = to_clip(p);
    // Local coordinates run from -half to +half, so the SDF is centred.
    out.local = (c - vec2<f32>(0.5, 0.5)) * grown;
    // The image sub-rect is a fraction of the RECT, not of the grown quad, so
    // an image node keeps sampling exactly the pixels it did before the
    // outset. Values run slightly past [0,1] over the ring; the sampler
    // clamps, and the SDF has already faded coverage to zero out there.
    out.uv    = (out.local + u.rect.zw * 0.5) / u.rect.zw;
    return out;
}

fn sd_rounded_box(p : vec2<f32>, b : vec2<f32>, r : f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_box(in : VsOut) -> @location(0) vec4<f32> {
    let half = u.rect.zw * 0.5;
    // Clamp the radius so a small box cannot produce a negative inner extent,
    // which would make the SDF fold inside out.
    let r = min(u.radius, min(half.x, half.y));
    let d = sd_rounded_box(in.local, half, r);

    // Analytic coverage across one pixel: a LINEAR ramp, not smoothstep.
    //
    // The fraction of a pixel covered by a straight edge at signed distance d is
    // linear in d. smoothstep is a cubic — a pleasant soft-edge *look*, but not
    // a coverage function.
    //
    // HONEST NOTE, so nobody re-derives this: spike S2 measured a ~0.21 px
    // stutter in a sliding box and this was the first hypothesis. Switching to
    // the linear ramp improved it only marginally (0.214 -> 0.205 px), so
    // smoothstep was NOT the cause. The change is kept because it is the correct
    // coverage function, not because it fixed the stutter. The stutter's root
    // cause is still open — see docs/spikes/S2-text-typography.md.
    // Bands from ONE distance: the stroke occupies [-w, 0] inside the edge,
    // the fill retreats behind it, and both share the outer AA ramp — so a
    // stroked box antialiases exactly where an unstroked one does.
    let w = u.effect.x;
    let edge_cov = clamp(0.5 - d, 0.0, 1.0);
    let fill_cov = clamp(0.5 - (d + w), 0.0, 1.0);
    let stroke_cov = max(edge_cov - fill_cov, 0.0);

    // Premultiplied within the node's own plane first. The bands are
    // disjoint, so addition is composition.
    let fill_pm = vec4<f32>(u.color.rgb, 1.0) * (u.color.a * fill_cov);
    let stroke_pm = vec4<f32>(u.stroke_color.rgb, 1.0) * (u.stroke_color.a * stroke_cov);
    let front = fill_pm + stroke_pm;

    // The shadow: this same silhouette, offset, its coverage ramp stretched
    // to `blur` design units. A linear ramp rather than a gaussian, for the
    // same reason the edge uses one — it is deterministic arithmetic on a
    // distance, where a gaussian invites per-backend texture or derivative
    // tricks (rule 1: never fwidth). Alpha 0 multiplies the whole term to
    // exact zeros, so a shadowless box renders exactly as it always has.
    let so = vec2<f32>(u.effect.y, u.effect.z);
    let sd = sd_rounded_box(in.local - so, half, r);
    let shadow_cov = clamp(0.5 - sd / max(u.effect.w, 1.0), 0.0, 1.0);
    let shadow_pm = vec4<f32>(u.shadow_color.rgb, 1.0) * (u.shadow_color.a * shadow_cov);

    // The shadow is knocked out under the node's own SILHOUETTE — not under
    // its painted alpha. An unfilled outlined box must not show its shadow
    // through the hole (the shadow is of the box, not visible through it),
    // and a translucent fill must not have a dark ghost bleeding up through
    // it. The same rule CSS box-shadow settled on, for the same reasons.
    return (front + shadow_pm * (1.0 - edge_cov)) * u.opacity;
}

// ---------------------------------------------------------------------------
// Text — one quad per glyph, sampling the coverage atlas.
//
// The atlas holds COVERAGE, a single channel. Colour is applied here and
// premultiplied here, once. Baking colour into the atlas would premultiply in
// the wrong space and make the cache per-colour.
// ---------------------------------------------------------------------------

struct GlyphVertex {
    @location(0) pos : vec2<f32>,   // design space, absolute
    @location(1) uv  : vec2<f32>,   // normalised atlas coordinates
};

@group(0) @binding(1) var atlas_tex : texture_2d<f32>;
@group(0) @binding(2) var atlas_smp : sampler;

@vertex
fn vs_text(v : GlyphVertex) -> VsOut {
    var out : VsOut;
    out.pos   = to_clip(v.pos);
    out.local = v.pos;
    out.uv    = v.uv;
    return out;
}

@fragment
fn fs_text(in : VsOut) -> @location(0) vec4<f32> {
    let coverage = textureSample(atlas_tex, atlas_smp, in.uv).r;
    let a = coverage * u.color.a * u.opacity;
    return vec4<f32>(u.color.rgb * a, a);   // premultiplied
}

// ---------------------------------------------------------------------------
// Unknown node kinds.
//
// A magenta placeholder, deliberately impossible to mistake for design intent.
// Drawing NOTHING would look fine and hide the problem, which is the dangerous
// behaviour (scene-format.md §3.4).
// ---------------------------------------------------------------------------

@fragment
fn fs_unknown(in : VsOut) -> @location(0) vec4<f32> {
    // Diagonal hatch, so it reads as "broken" rather than as a magenta box
    // someone might have meant.
    let stripe = fract((in.local.x + in.local.y) * 0.05);
    let a = select(0.35, 0.85, stripe < 0.5) * u.opacity;
    return vec4<f32>(vec3<f32>(1.0, 0.0, 1.0) * a, a);
}

// ---------------------------------------------------------------------------
// Images.
//
// The texture holds PREMULTIPLIED RGBA — pmg-media multiplies at decode, so
// compositing here needs no conversion and the blend state is the same
// source-over every other pipeline uses. Sampling straight alpha and blending
// it as premultiplied is the classic way to get a dark fringe on every soft
// edge, and it is invisible until the graphic is over live video.
//
// `u.uv` is the sub-rectangle chosen by `fit` at BUILD time, so this shader
// does one lookup and knows nothing about aspect ratios.
// ---------------------------------------------------------------------------

@group(1) @binding(0) var img_tex : texture_2d<f32>;
@group(1) @binding(1) var img_smp : sampler;

fn srgb_to_linear(c : f32) -> f32 {
    // The exact piecewise curve, matching srgb_to_linear() on the Rust side and
    // linear_to_srgb() below. The linear toe is what soft edges are made of.
    if (c <= 0.0404500) {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

@fragment
fn fs_image(in : VsOut) -> @location(0) vec4<f32> {
    let uv = u.uv.xy + in.uv * u.uv.zw;
    let enc = textureSample(img_tex, img_smp, uv);

    // Decoded pixels are premultiplied AND sRGB-encoded, and the scene
    // composites in linear (ADR 0012). Un-premultiply before linearizing:
    // srgb_to_linear(a * c) is not a * srgb_to_linear(c), and the error is
    // largest at exactly the low alphas a soft edge is made of.
    var texel = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (enc.a > 0.0) {
        let straight = enc.rgb / enc.a;
        let lin = vec3<f32>(
            srgb_to_linear(straight.r),
            srgb_to_linear(straight.g),
            srgb_to_linear(straight.b),
        );
        texel = vec4<f32>(lin * enc.a, enc.a);
    }

    // Rounded corners still apply: an image node with a radius is a rounded
    // picture, not a rectangle with rounded paint over it.
    let half = u.rect.zw * 0.5;
    let r = min(u.radius, min(half.x, half.y));
    let d_corner = sd_rounded_box(in.local, half, r);

    // The fit window. Scale-to-fit maps the source onto a window SMALLER than
    // the rect (uv leaves [0,1] over the letterbox), and coverage must fade
    // there as analytically as it does at the corners — fwidth stays banned.
    // Convert the uv overshoot back to design units through the mapping's own
    // scale; for cover and stretch uv stays inside [0,1] across the rect, the
    // distance below never exceeds the plain box SDF, and the max() with the
    // corner distance changes nothing — measured as identical, not hoped.
    let unit = u.rect.zw / u.uv.zw; // design units per uv unit, per axis
    let over = max(-uv, uv - vec2<f32>(1.0, 1.0)) * unit;
    let d_fit = max(over.x, over.y);
    let coverage = clamp(0.5 - max(d_corner, d_fit), 0.0, 1.0);

    // Already premultiplied, so opacity scales all four channels together.
    return texel * coverage * u.opacity;
}

// ---------------------------------------------------------------------------
// Encode: linear light to the output transfer function.
//
// The scene composites into an Rgba16Float target in LINEAR light (ADR 0012),
// and this is the "encode once, at output" half of that rule. One full-screen
// triangle, no vertex buffer.
//
// Applied to the colour only. Alpha is coverage — a geometric fraction that was
// never encoded — and putting it through a transfer function here would undo
// the key, which must stay a monotone alpha signal.
// ---------------------------------------------------------------------------

@group(0) @binding(1) var scene_tex : texture_2d<f32>;
@group(0) @binding(2) var scene_smp : sampler;

struct EncodeOut {
    @builtin(position) pos : vec4<f32>,
    @location(0) uv : vec2<f32>,
};

@vertex
fn vs_encode(@builtin(vertex_index) i : u32) -> EncodeOut {
    // One oversized triangle rather than two triangles: no shared edge, so no
    // chance of a seam from differing interpolation on either side of it.
    var out : EncodeOut;
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    out.pos = vec4<f32>(x, y, 0.0, 1.0);
    out.uv  = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

fn linear_to_srgb(c : f32) -> f32 {
    // The exact piecewise inverse, matching srgb_to_linear() on the Rust side.
    // The linear toe is what an antialiased edge is made of.
    if (c <= 0.0031308) {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

@fragment
fn fs_encode(in : EncodeOut) -> @location(0) vec4<f32> {
    let px = textureSample(scene_tex, scene_smp, in.uv);
    let a = px.a;
    // Un-premultiply, encode, re-premultiply. Encoding the premultiplied value
    // directly is wrong because the transfer function is not linear:
    // encode(a*c) != a*encode(c), and the error is largest at the low alphas an
    // antialiased edge is entirely composed of.
    if (a <= 0.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let straight = px.rgb / a;
    let enc = vec3<f32>(
        linear_to_srgb(straight.r),
        linear_to_srgb(straight.g),
        linear_to_srgb(straight.b),
    );
    return vec4<f32>(enc * a, a);
}
