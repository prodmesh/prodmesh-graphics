// Parity corpus shaders — spike S1.
//
// DETERMINISM RULES. Every one of these exists because breaking it would make
// native and browser output differ *by specification*, not by accident.
//
//  1. NO fwidth / dpdx / dpdy. Derivative granularity (fine vs coarse) is
//     implementation-defined, so antialiasing built on derivatives differs
//     between backends legitimately. Pixel scale arrives as a uniform instead.
//
//  2. NO MSAA anywhere. Sample patterns are unspecified across backends.
//     All antialiasing here is analytic.
//
//  3. The target is Rgba8Unorm, NOT Rgba8UnormSrgb. Automatic sRGB conversion
//     is one more thing that could differ; we encode explicitly, in-shader, or
//     not at all.
//
//  4. No reliance on sampler filtering defaults — every sampler is fully
//     specified on the Rust side.

struct Uniforms {
    // Rect in normalised device space: centre and half-extent.
    center      : vec2<f32>,
    half_extent : vec2<f32>,
    color       : vec4<f32>,
    // Corner radius, in the same units as half_extent.
    radius      : f32,
    // How many of those units one pixel spans. This replaces fwidth() and is
    // what makes the antialiased edge reproducible across backends.
    px_per_unit : f32,
    _pad        : vec2<f32>,
};

@group(0) @binding(0) var<uniform> u : Uniforms;

struct VsOut {
    @builtin(position) pos : vec4<f32>,
    @location(0)       uv  : vec2<f32>,
};

// ---------------------------------------------------------------------------
// Stage A — solid triangle.
//
// Tests raster coverage rules and the vertex path. Fixed vertices so there is
// no buffer-layout variable in the comparison.
// ---------------------------------------------------------------------------

@vertex
fn vs_triangle(@builtin(vertex_index) i : u32) -> VsOut {
    var p = array<vec2<f32>, 3>(
        vec2<f32>(-0.7, -0.6),
        vec2<f32>( 0.7, -0.6),
        vec2<f32>( 0.0,  0.75),
    );
    var out : VsOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    out.uv  = p[i];
    return out;
}

@fragment
fn fs_solid(in : VsOut) -> @location(0) vec4<f32> {
    return u.color;
}

// ---------------------------------------------------------------------------
// Stages B and C — a fullscreen quad, so the fragment shader owns the geometry.
// ---------------------------------------------------------------------------

@vertex
fn vs_fullscreen(@builtin(vertex_index) i : u32) -> VsOut {
    // Two triangles, wound consistently. Explicit rather than a clever
    // bit-twiddled fullscreen trick — legibility beats brevity in the one
    // shader whose output is the measurement.
    var p = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>( 1.0, -1.0), vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0,  1.0), vec2<f32>( 1.0, -1.0), vec2<f32>( 1.0,  1.0),
    );
    var out : VsOut;
    out.pos = vec4<f32>(p[i], 0.0, 1.0);
    out.uv  = p[i];
    return out;
}

// Signed distance to a rounded box. Standard, and importantly it is exact —
// no iteration, no approximation whose error could differ between compilers.
fn sd_rounded_box(p : vec2<f32>, b : vec2<f32>, r : f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r, r);
    return length(max(q, vec2<f32>(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - r;
}

// ---------------------------------------------------------------------------
// Stage B — analytically antialiased rounded rectangle.
//
// THE stage that matters. This is our own AA math, and it is the thing shared
// code is supposed to make identical. Coverage is computed from the exact
// signed distance and the uniform pixel scale — never from a derivative.
//
// Output is PREMULTIPLIED, matching the working space in ADR 0012.
// ---------------------------------------------------------------------------

@fragment
fn fs_rounded_rect(in : VsOut) -> @location(0) vec4<f32> {
    let d = sd_rounded_box(in.uv - u.center, u.half_extent, u.radius);

    // Coverage across one pixel of the edge. smoothstep is a defined cubic, so
    // it is reproducible; the half-pixel band is derived from px_per_unit.
    let half_px = 0.5 * u.px_per_unit;
    let coverage = 1.0 - smoothstep(-half_px, half_px, d);

    let a = clamp(coverage, 0.0, 1.0) * u.color.a;
    return vec4<f32>(u.color.rgb * a, a);   // premultiplied
}

// ---------------------------------------------------------------------------
// Stage C — textured quad over a CPU-generated alpha bitmap.
//
// This is the glyph-atlas path without the text stack. Glyphs are rasterized on
// the CPU in pmg-text, so they are identical between targets by construction;
// what the GPU does differently is SAMPLE them. That is what this measures.
// ---------------------------------------------------------------------------

@group(0) @binding(1) var atlas_tex : texture_2d<f32>;
@group(0) @binding(2) var atlas_smp : sampler;

@fragment
fn fs_atlas(in : VsOut) -> @location(0) vec4<f32> {
    // uv is in [-1,1]; map to [0,1] with an explicit flip so orientation is
    // stated rather than inherited from a convention.
    let t = vec2<f32>((in.uv.x + 1.0) * 0.5, 1.0 - (in.uv.y + 1.0) * 0.5);
    let a = textureSample(atlas_tex, atlas_smp, t).r * u.color.a;
    return vec4<f32>(u.color.rgb * a, a);   // premultiplied
}
