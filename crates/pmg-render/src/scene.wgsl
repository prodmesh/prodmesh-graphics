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

@vertex
fn vs_box(@builtin(vertex_index) i : u32) -> VsOut {
    var corner = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    );
    let c = corner[i];
    let p = u.rect.xy + c * u.rect.zw;

    var out : VsOut;
    out.pos   = to_clip(p);
    // Local coordinates run from -half to +half, so the SDF is centred.
    out.local = (c - vec2<f32>(0.5, 0.5)) * u.rect.zw;
    out.uv    = c;
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

    // Analytic coverage across one pixel. smoothstep is a defined cubic, so it
    // is reproducible; the band comes from the uniform, never a derivative.
    let coverage = 1.0 - smoothstep(-0.5, 0.5, d);

    let a = clamp(coverage, 0.0, 1.0) * u.color.a * u.opacity;
    return vec4<f32>(u.color.rgb * a, a);   // premultiplied
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
