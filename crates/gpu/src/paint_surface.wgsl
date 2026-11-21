//! A stroke's paint surface on the GPU: one instanced draw lays every dab of a frame into it,
//! and nothing comes back to the CPU until the stroke commits.
//!
//! This is the first half of the GPU painting engine (`paint_surface.rs`): the same per-pixel
//! tip maths as `crates/paint/src/render.rs`'s `rasterize_into` and `brush_stamp.wgsl`, laid by
//! instanced quads into two attachments — coverage (accumulated the way `CoverageMap::accumulate`
//! builds it up: `c ← c + v·(ceil − c)` through fixed-function blending) and Wet Edges' depth
//! (`max`). The pattern / grain / sampled-tip paths arrive in the next slice; until then the host
//! offers only the dabs this shader draws and keeps the rest on the CPU.

// The stroke-level constants: surface width and height in pixels, the opacity ceiling `ceil`, the
// paint colour's untouched alpha.
struct Style {
    size: vec4<f32>,
    // The stroke's colour (straight, 0..1 in r, g, b); with a patterned brush, the colour a pixel
    // picks from the pattern, sampled in the next slice.
    color: vec4<f32>,
    // Texture: scale, depth, unused, unused.
    tex: vec4<f32>,
    // Texture flags: present, each_tip, coordinate mode (0 canvas, 1 stroke, 2 tip), c3_math.
    tex_flags: vec4<u32>,
    // The texture's combine mode (see `mask_combine`), and the stroke-locked origin.
    tex_mode: vec4<u32>,
    tex_origin: vec4<f32>,
    // The sampled tip: present, mip levels (0 = none), unused, unused.
    tip_flags: vec4<u32>,
};

// The stroke texture, sampled bilinearly with wrap (the CPU's `sample_bilinear_wrap`).
@group(0) @binding(2) var stroke_tex: texture_2d<f32>;
@group(0) @binding(3) var stroke_sampler: sampler;
// The brush's tip bitmap (a sampled tip), 8-bit with its mip chain, sampled the way `Mips`
// samples it: clamp at the edge, linear in between.
@group(0) @binding(4) var tip_tex: texture_2d<f32>;
@group(0) @binding(5) var tip_sampler: sampler;

// One dab, as the brush engine resolved it (`StampJob` + its rectangle in surface pixels).
struct Dab {
    // x0, y0, x1, y1: the quad this dab is drawn over, in surface pixels.
    rect: vec4<f32>,
    // centre x, centre y (snapped for an aliased dab), radius, flow alpha.
    center: vec4<f32>,
    // angle (radians), proj_scale, proj_angle, hardness.
    misc: vec4<f32>,
    // flip_x, flip_y, the dab's texture depth, the sampled tip's level-0 scale (`tw / (2 r)`).
    flip: vec4<f32>,
    // c3_tip_math, aliased, square (Block), triangle.
    flags: vec4<u32>,
    // small (area-sampled tip), wet_edges (single-dab Wet Edges), sampled tip's mip level,
    // sampled (1 = a bitmap tip).
    flags2: vec4<u32>,
    // Dab-invariant geometry from the CPU: roundness, scaled radius, reach², small reach².
    geom: vec4<f32>,
    // Where this dab's quad sits on the surface (`rect` minus this is the quad), and the
    // document-pixel origin a fragment adds back to `@builtin(position)`; zero for a surface
    // that *is* the document.
    atlas: vec4<f32>,
    // The dab's own straight colour (rgb): colour dynamics give every dab its own, and the
    // folded frame's colour lane is what averages them.
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> s: Style;
@group(0) @binding(1) var<storage, read> dabs: array<Dab>;

// The CPU's 4×4 sub-pixel grid for the area coverage of a small tip.
const SUBPIXEL = array<f32, 4>(-0.375, -0.125, 0.125, 0.375);
const TIP_EDGE_PX: f32 = 1.2;

// Photoshop's measured round tip: solid core `r0 · R`, Gaussian shoulder `s · R` (see `lib.rs`).
fn tip_shape(radius_in: f32, hardness: f32) -> vec2<f32> {
    var table = array<vec2<f32>, 11>(
        vec2<f32>(0.00, 0.664),
        vec2<f32>(0.14, 0.58),
        vec2<f32>(0.28, 0.50),
        vec2<f32>(0.40, 0.44),
        vec2<f32>(0.52, 0.36),
        vec2<f32>(0.63, 0.29),
        vec2<f32>(0.70, 0.24),
        vec2<f32>(0.78, 0.19),
        vec2<f32>(0.84, 0.15),
        vec2<f32>(0.89, 0.11),
        vec2<f32>(1.00, 0.0),
    );
    let h = clamp(hardness, 0.0, 1.0) * 10.0;
    let i = min(u32(floor(h)), 9u);
    let t = h - f32(i);
    let a = table[i];
    let b = table[i + 1u];
    let r0 = a.x + (b.x - a.x) * t;
    let sd = a.y + (b.y - a.y) * t;
    let radius = max(radius_in, 0.0);
    let edge = max(min(TIP_EDGE_PX, 0.3 * radius), 0.25);
    let core = max(min(r0 * radius, radius - edge), 0.0);
    let shoulder = max(max(sd * radius, edge), 1e-3);
    return vec2<f32>(core, shoulder);
}

// `exp(-u)` as `(1 − u/64)⁶⁴` by six squarings: exact arithmetic, the same as the CPU's.
fn soft_exp(u: f32) -> f32 {
    var g = clamp(1.0 - u / 64.0, 0.0, 1.0);
    for (var i = 0; i < 6; i = i + 1) {
        g = g * g;
    }
    return g;
}

fn tip_falloff(d: f32, radius: f32, hardness: f32) -> f32 {
    let shape = tip_shape(radius, hardness);
    if (d <= shape.x) {
        return 1.0;
    }
    let x = (d - shape.x) / shape.y;
    return soft_exp(x * x);
}

fn dab_coverage(d: f32, radius: f32, hardness: f32) -> f32 {
    let shape = tip_shape(radius, hardness);
    if (d >= shape.x + 2.6 * shape.y + 0.5) {
        return 0.0;
    }
    return tip_falloff(d, radius, hardness);
}

// C3's normalised Gaussian (k = 2.5), hard tip's 1 px linear AA and its sub-pixel step.
fn c3_tip_falloff(distance: f32, radius: f32, hardness: f32, subpixel: bool) -> f32 {
    if (hardness >= 0.999) {
        if (subpixel) {
            return select(0.0, 1.0, distance <= radius);
        }
        return clamp(radius - distance + 0.5, 0.0, 1.0);
    }
    let h = clamp(hardness, 0.0, 0.999);
    let t = clamp((distance / max(radius, 0.0005) - h) / (1.0 - h), 0.0, 1.0);
    let tail = exp(-2.5);
    return (exp(-2.5 * t * t) - tail) / (1.0 - tail);
}

fn tip_coverage(c3: bool, aliased: bool, dd: f32, rm: f32, ro: f32, r: f32, hardness: f32) -> f32 {
    if (aliased) {
        return select(0.0, 1.0, dd <= rm);
    }
    if (c3) {
        return c3_tip_falloff(dd / ro, r, hardness, false);
    }
    return dab_coverage(dd, rm, hardness);
}

fn triangle_mask(u: f32, v: f32, radius: f32, roundness: f32, hardness: f32, subpixel: bool) -> f32 {
    let qx = u / max(radius, 0.1);
    let qy = -v / max(radius * roundness, 0.1);
    let edge = min(min((qy + 1.0 - 2.0 * qx) / sqrt(5.0), 1.0 - qy), (qy + 2.0 * qx + 1.0) / sqrt(5.0));
    if (edge <= 0.0) {
        return 0.0;
    }
    let distance = clamp(1.0 - edge / 0.4472136, 0.0, 1.0);
    return c3_tip_falloff(distance * radius, radius, hardness, subpixel);
}

fn smoothstep01(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Wet Edges: half the paint at a dab's centre, all of it at the rim.
fn wet_factor(rn: f32) -> f32 {
    return 0.5 + 0.5 * smoothstep01(0.5, 1.0, rn);
}

// A pixel offset from the dab centre in tip space: to y-up, project, rotate, flip.
fn to_tip(dx: f32, dy: f32, ax: f32, ay: f32, k: f32, sn: f32, cs: f32, fx: f32, fy: f32) -> vec2<f32> {
    var ux = dx;
    var uy = -dy;
    if (k != 0.0) {
        let t = (ux * ax + uy * ay) * k;
        ux = ux + t * ax;
        uy = uy + t * ay;
    }
    return vec2<f32>((ux * cs + uy * sn) * fx, (-ux * sn + uy * cs) * fy);
}

struct VOut {
    @builtin(position) clip: vec4<f32>,
    // The dab this fragment belongs to (flat, so every pixel of a quad reads the same one).
    @location(0) @interpolate(flat) index: u32,
};

// The fragment's side of the same varying: `@builtin(position)` arrives as its own argument.
struct FIn {
    @location(0) @interpolate(flat) index: u32,
};

fn mixd(a: f32, b: f32, d: f32) -> f32 {
    return a + (b - a) * d;
}

fn divide(p: f32, s: f32) -> f32 {
    if (s <= 0.0) {
        return 1.0;
    }
    return min(p / s, 1.0);
}

// The CPU's `mask_combine`: how the texture's value `t` (at `d` = the texture's own depth) meets
// the dab's coverage `v`. Same expressions, same clamps.
fn mask_combine(mode: u32, v: f32, t: f32, d: f32) -> f32 {
    if (v <= 0.0) {
        return 0.0;
    }
    var r = v;
    switch mode {
        // Normal
        case 0u: { r = v + d * t - v * d * t; }
        // Multiply
        case 1u: { r = v * (1.0 - d + d * t); }
        // Subtract
        case 2u: { r = max(v - d * (1.0 - t), 0.0); }
        // Lighten
        case 3u: { r = mixd(v, max(v, t), d); }
        // Difference
        case 4u: { r = mixd(v, abs(v - t), d); }
        // Divide
        case 5u: { r = mixd(v, divide(v, t), d); }
        // Darken
        case 6u: { r = min(v, 1.0 - d + d * t); }
        // Overlay
        case 7u: {
            let o = select(1.0 - 2.0 * (1.0 - v) * (1.0 - t), 2.0 * v * t, v < 0.5);
            r = mixd(v, o, d);
        }
        // ColorDodge
        case 8u: { r = mixd(v, min(v / max(1.0 - t, 1e-3), 1.0), d); }
        // ColorBurn
        case 9u: { r = mixd(v, 1.0 - min((1.0 - v) / max(t, 1e-3), 1.0), d); }
        // LinearBurn
        case 10u: { r = mixd(v, max(v + t - 1.0, 0.0), d); }
        // HardMix
        case 11u: { r = mixd(v, select(1.0, 0.0, v + t > 1.0), d); }
        // LinearHeight
        case 12u: { r = v * clamp((t - (1.0 - d * v)) * 4.0 + 0.5, 0.0, 1.0); }
        // Height
        case 13u: { r = select(0.0, v, t >= 1.0 - d * v); }
        default: { r = v; }
    }
    return clamp(r, 0.0, 1.0);
}

// The stroke texture at a document pixel: the CPU's `texture_at_point`, in its three coordinate
// modes, with the bilinear wrap-around read the GPU's sampler does natively.
fn texture_at(px: vec2<f32>, d: Dab) -> f32 {
    let scale = max(s.tex.x, 0.01);
    var at = px;
    let mode = s.tex_flags.z;
    if (mode == 1u) {
        at = at - vec2<f32>(s.tex_origin.x, s.tex_origin.y);
    } else if (mode == 2u) {
        // Tip-local: the dab's own frame, y up, rotated and flipped as the tip.
        let dx = at.x - d.center.x;
        let dy = -(at.y - d.center.y);
        let c3 = s.tex_flags.w != 0u;
        let angle = select(d.misc.x, -d.misc.x, c3);
        let sn = sin(angle);
        let cs = cos(angle);
        let u = (dx * cs + dy * sn) * d.flip.x;
        let v = (-dx * sn + dy * cs) * d.flip.y;
        at.x = u + d.center.z;
        at.y = select(v, -v, c3) / max(d.geom.x, 0.001) + d.center.z;
    }
    let w = f32(textureDimensions(stroke_tex).x);
    let h = f32(textureDimensions(stroke_tex).y);
    let uv = vec2<f32>(at.x / scale / w, at.y / scale / h);
    return textureSampleLevel(stroke_tex, stroke_sampler, uv, 0.0).r;
}

// A sampled tip, as `Mips` samples it: the bitmap's level 0 sampled at texel centres, so the
// coordinates clamp a half texel inside the edge (`c3_sampled_mask` does it itself, hardware
// clamp-to-edge is the same thing for the plain path).
fn tip_sample(level: u32, uv: vec2<f32>, c3_clamped: bool) -> f32 {
    let dims = vec2<f32>(f32(textureDimensions(tip_tex, 0).x), f32(textureDimensions(tip_tex, 0).y));
    var at = uv;
    if (c3_clamped) {
        let lo = vec2<f32>(0.5 / dims.x, 0.5 / dims.y);
        let hi = vec2<f32>(1.0 - lo.x, 1.0 - lo.y);
        at = clamp(at, lo, hi);
    }
    return textureSampleLevel(tip_tex, tip_sampler, at, f32(level)).r;
}

// `c3_sampled_mask`: level 0, a nine-tap blur weighted by how soft the tip is, an edge falloff
// and a hardness curve — the reference application's sampled-tip maths, tile for tile.
fn c3_sampled_mask(uv: vec2<f32>, hardness: f32) -> f32 {
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return 0.0;
    }
    let dims = vec2<f32>(f32(textureDimensions(tip_tex, 0).x), f32(textureDimensions(tip_tex, 0).y));
    let base = tip_sample(0u, uv, true);
    let hrd = clamp(hardness, 0.0, 1.0);
    if (hrd >= 0.999) {
        return base;
    }
    let boost = clamp((1.0 - hrd) * 5.0, 0.0, 1.0);
    let rx = min((0.6 + boost * 6.4) / dims.x, 0.125);
    let ry = min((0.6 + boost * 6.4) / dims.y, 0.125);
    var blur = base * 0.28;
    blur = blur + (tip_sample(0u, uv + vec2<f32>(rx, 0.0), true) + tip_sample(0u, uv - vec2<f32>(rx, 0.0), true)) * 0.11;
    blur = blur + (tip_sample(0u, uv + vec2<f32>(0.0, ry), true) + tip_sample(0u, uv - vec2<f32>(0.0, ry), true)) * 0.11;
    blur = blur + (tip_sample(0u, uv + vec2<f32>(rx, ry), true) + tip_sample(0u, uv + vec2<f32>(-rx, ry), true)) * 0.07;
    blur = blur + (tip_sample(0u, uv + vec2<f32>(rx, -ry), true) + tip_sample(0u, uv + vec2<f32>(-rx, -ry), true)) * 0.07;
    let shaped = clamp(min(base, base + (blur - base) * boost), 0.0, 1.0);
    let e = max(abs(uv.x * 2.0 - 1.0), abs(uv.y * 2.0 - 1.0));
    return pow(shaped, 2.2 - 1.2 * hrd) * c3_tip_falloff(e, 1.0, hrd, false);
}

@vertex
fn vs(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let d = dabs[ii];
    let c = corners[vi];
    let px = vec2<f32>(d.rect.x + c.x * (d.rect.z - d.rect.x), d.rect.y + c.y * (d.rect.w - d.rect.y)) - d.atlas.xy;
    var out: VOut;
    out.clip = vec4<f32>(px.x / s.size.x * 2.0 - 1.0, 1.0 - px.y / s.size.y * 2.0, 0.0, 1.0);
    out.index = ii;
    return out;
}

struct Frag {
    // Coverage: `ceil·val` with the mask in alpha. Blending with (one, one − src alpha) is the
    // CPU's `c ← c + v·(ceil − c)`, one dab at a time, in submission order. Its colour channel is
    // what the host reads; the alpha channel's own blend keeps it out of the way.
    @location(0) cov: vec4<f32>,
    // Paint: `ceil·colour·val`, blended with the same factor, so its colour ends as
    // `colour × coverage` — the CPU's `col` lane, premultiplied.
    @location(1) paint: vec4<f32>,
    // `1 − rn`, blended with `max`.
    @location(2) depth: f32,
};

@fragment
fn fs(input: FIn, @builtin(position) pos: vec4<f32>) -> Frag {
    let d = dabs[input.index];
    let r = d.center.z;
    let alpha = d.center.w;
    let hardness = d.misc.w;
    let fx = d.flip.x;
    let fy = d.flip.y;
    let c3 = d.flags.x != 0u;
    let aliased = d.flags.y != 0u;
    let square = d.flags.z != 0u;
    let triangle = d.flags.w != 0u;
    let small = d.flags2.x != 0u;

    let ro = d.geom.x;
    let rm = d.geom.y;
    let reach2 = d.geom.z;
    let small_reach2 = d.geom.w;

    // Angles turn clockwise on the screen under C3, counter-clockwise otherwise.
    let signed_angle = select(d.misc.x, -d.misc.x, c3);
    let sn = sin(signed_angle);
    let cs = cos(signed_angle);
    // Brush Projection: stretch sampling along the tilt direction, foreshortening the tip.
    let proj_scale = d.misc.y;
    let k = select(0.0, 1.0 / max(proj_scale, 0.05) - 1.0, proj_scale < 0.999);
    let ax = cos(d.misc.z);
    let ay = sin(d.misc.z);

    // The pixel centre, in document pixels, exactly as the CPU takes it.
    let doc = vec2<f32>(pos.x + d.atlas.z, pos.y + d.atlas.w);
    let dx = doc.x - d.center.x;
    let dy = doc.y - d.center.y;
    let tip = to_tip(dx, dy, ax, ay, k, sn, cs, fx, fy);
    let u = tip.x;
    let v = tip.y;

    var val = 0.0;
    var rn = 0.0;

    if (triangle) {
        var value = 0.0;
        if (small) {
            var sum = 0.0;
            for (var sy = 0u; sy < 4u; sy = sy + 1u) {
                for (var sx = 0u; sx < 4u; sx = sx + 1u) {
                    let st = to_tip(dx + SUBPIXEL[sx], dy + SUBPIXEL[sy], ax, ay, k, sn, cs, fx, fy);
                    sum = sum + triangle_mask(st.x, st.y, r, ro, hardness, true);
                }
            }
            value = sum / 16.0;
        } else {
            value = triangle_mask(u, v, r, ro, hardness, false);
        }
        val = select(value, select(0.0, 1.0, value >= 0.5), aliased);
        rn = 1.0 - value;
    } else if (aliased && square) {
        // Block: the whole-pixel square, hard (the Eraser's Block mode).
        if (abs(dx) >= r || abs(dy) >= r) {
            discard;
        }
        val = 1.0;
        rn = 0.0;
    } else if (small) {
        let ru = u * ro;
        let d2 = ru * ru + v * v;
        if (d2 >= small_reach2) {
            discard;
        }
        var sum = 0.0;
        for (var sy = 0u; sy < 4u; sy = sy + 1u) {
            for (var sx = 0u; sx < 4u; sx = sx + 1u) {
                let st = to_tip(dx + SUBPIXEL[sx], dy + SUBPIXEL[sy], ax, ay, k, sn, cs, fx, fy);
                let sr = st.x * ro;
                let sd = sqrt(sr * sr + st.y * st.y);
                sum = sum + select(tip_falloff(sd, rm, hardness), c3_tip_falloff(sd / ro, r, hardness, true), c3);
            }
        }
        val = sum / 16.0;
        rn = sqrt(d2) / rm;
    } else if (d.flags2.w != 0u) {
        // A sampled tip: the bitmap drawn `2 r` pixels across, in the tip's own coordinates.
        let tw = f32(textureDimensions(tip_tex, 0).x);
        let th = f32(textureDimensions(tip_tex, 0).y);
        let inv = d.flip.w;
        let px = u * inv + tw * 0.5;
        let py = -(v / max(ro, 0.001)) * inv + th * 0.5;
        if (px < -1.0 || py < -1.0 || px > tw + 1.0 || py > th + 1.0) {
            discard;
        }
        let uv = vec2<f32>(px / tw, py / th);
        var mask = 0.0;
        if (c3) {
            mask = c3_sampled_mask(uv, hardness);
        } else {
            mask = tip_sample(d.flags2.z, uv, false);
        }
        val = select(mask, select(0.0, 1.0, mask >= 0.5), aliased);
        rn = 1.0 - mask;
    } else {
        let ru = u * ro;
        let d2 = ru * ru + v * v;
        if (d2 >= reach2) {
            discard;
        }
        let dd = sqrt(d2);
        val = tip_coverage(c3, aliased, dd, rm, ro, r, hardness);
        rn = dd / rm;
    }

    if (val <= 0.0) {
        discard;
    }
    // Per-tip texture: the pattern replaces the coverage through the brush's combine mode, exactly
    // where the CPU applies `tex_tip` (after the tip shape, before the stroke's own work), at the
    // dab's own depth (`d.flip.z`, jittered per tip), as `texture.combine` takes it.
    if (s.tex_flags.x != 0u && s.tex_flags.y != 0u) {
        val = mask_combine(s.tex_mode.x, val, texture_at(doc, d), d.flip.z);
    }
    var out: Frag;
    let cov = val * alpha;
    out.cov = vec4<f32>(s.size.z * cov, 0.0, 0.0, cov);
    out.paint = vec4<f32>(d.color.rgb * cov, cov);
    // The stroke keeps the deepest any dab reaches each pixel (`CoverageMap::depth`) and applies
    // Wet Edges over the accumulated coverage itself, so a dab laid for a stroke is never shaped
    // by the rim here (the CPU path's `wet` only exists when it gets no depth buffer).
    out.depth = clamp(1.0 - rn, 0.0, 1.0);
    return out;
}
