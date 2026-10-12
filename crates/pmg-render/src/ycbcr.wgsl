// Linear premultiplied RGBA -> 10-bit 4:2:2 YCbCr, packed as v210.
//
// The GPU execution of pmg_color::v210::encode, which is the reference and the
// test oracle: same arithmetic, same rounding, same padding. Change one and the
// test in tests/ycbcr.rs says so.
//
// A fragment pass, not a compute one, on purpose: the node's device is
// requested with WebGL2-level limits, which allow no compute, and raising them
// for one pass would make every other pass's portability a question again.
// Each fragment of an R32Uint target IS one v210 word; two targets carry the
// fill and the key from one draw.

struct Levels {
    a : vec4<f32>,   // kr, kb, y_off, y_scale
    b : vec4<f32>,   // c_scale, y_min, y_max, c_min
    c : vec4<f32>,   // c_max, k_off, k_scale, k_min
    d : vec4<f32>,   // k_max, -, -, -
    size : vec4<u32>, // width, height, -, -
};

@group(0) @binding(0) var<uniform> lv : Levels;
@group(0) @binding(1) var linear_tex : texture_2d<f32>;

@vertex
fn vs_ycbcr(@builtin(vertex_index) i : u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn px(x : u32, y : u32) -> vec4<f32> {
    // Padding past the picture is transparent black: black fill, keyed out.
    if (x >= lv.size.x) {
        return vec4<f32>(0.0);
    }
    return textureLoad(linear_tex, vec2<i32>(i32(x), i32(y)), 0);
}

// The sRGB curve, identical to scene.wgsl's linear_to_srgb, with the clamp the
// reference applies.
fn oetf(v_in : f32) -> f32 {
    let v = clamp(v_in, 0.0, 1.0);
    if (v <= 0.0031308) {
        return 12.92 * v;
    }
    return 1.055 * pow(v, 1.0 / 2.4) - 0.055;
}

// (Y', Cb', Cr') premultiplied over black.
fn fill(p : vec4<f32>) -> vec3<f32> {
    let a = clamp(p.a, 0.0, 1.0);
    if (a <= 0.0) {
        return vec3<f32>(0.0);
    }
    let r = oetf(p.r / a) * a;
    let g = oetf(p.g / a) * a;
    let b = oetf(p.b / a) * a;
    let kr = lv.a.x;
    let kb = lv.a.y;
    let y = kr * r + (1.0 - kr - kb) * g + kb * b;
    return vec3<f32>(y, (b - y) / (2.0 * (1.0 - kb)), (r - y) / (2.0 * (1.0 - kr)));
}

// Round half up — WGSL's round() is half-to-even and would disagree with the
// reference on exactly the values a ramp is made of.
fn code(v : f32, lo : f32, hi : f32) -> u32 {
    return u32(clamp(floor(v + 0.5), lo, hi));
}

fn luma(p : vec4<f32>) -> u32 {
    return code(lv.a.z + lv.a.w * fill(p).x, lv.b.y, lv.b.z);
}

fn cb(p0 : vec4<f32>, p1 : vec4<f32>) -> u32 {
    return code(512.0 + lv.b.x * (fill(p0).y + fill(p1).y) * 0.5, lv.b.w, lv.c.x);
}

fn cr(p0 : vec4<f32>, p1 : vec4<f32>) -> u32 {
    return code(512.0 + lv.b.x * (fill(p0).z + fill(p1).z) * 0.5, lv.b.w, lv.c.x);
}

// The key: alpha, linear, in the key's own range. No matrix, no curve.
fn key(p : vec4<f32>) -> u32 {
    return code(lv.c.y + lv.c.z * clamp(p.a, 0.0, 1.0), lv.c.w, lv.d.x);
}

fn word(a : u32, b : u32, c : u32) -> u32 {
    return a | (b << 10u) | (c << 20u);
}

struct Out {
    @location(0) fill : u32,
    @location(1) key : u32,
};

@fragment
fn fs_ycbcr(@builtin(position) pos : vec4<f32>) -> Out {
    let wx = u32(pos.x);
    let y = u32(pos.y);
    let base = (wx / 4u) * 6u;
    let p0 = px(base, y);
    let p1 = px(base + 1u, y);
    let p2 = px(base + 2u, y);
    let p3 = px(base + 3u, y);
    let p4 = px(base + 4u, y);
    let p5 = px(base + 5u, y);
    var out : Out;
    switch (wx % 4u) {
        case 0u: {
            out.fill = word(cb(p0, p1), luma(p0), cr(p0, p1));
            out.key = word(512u, key(p0), 512u);
        }
        case 1u: {
            out.fill = word(luma(p1), cb(p2, p3), luma(p2));
            out.key = word(key(p1), 512u, key(p2));
        }
        case 2u: {
            out.fill = word(cr(p2, p3), luma(p3), cb(p4, p5));
            out.key = word(512u, key(p3), 512u);
        }
        default: {
            out.fill = word(luma(p4), cr(p4, p5), luma(p5));
            out.key = word(key(p4), 512u, key(p5));
        }
    }
    return out;
}
