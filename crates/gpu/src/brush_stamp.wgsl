// Brush dab stamping for the computed-tip paths: the C3 tip maths (`c3_tip_math`) and the legacy
// Photoshop-table tip, each with the aliased, Block, Triangle and Wet Edges variants.
//
// It evaluates the same expressions, in the same order, as `crates/paint/src/render.rs`
// `rasterize_into` (the CPU path) and `render_reference.rs` (the frozen oracle), so the GPU and
// the CPU agree pixel for pixel on what a dab covers. Sampled tips (mip pyramids), per-tip
// texture, Noise, Grain and Dual Brush keep the CPU path: the host asks `StampDab::unsupported`
// before dispatching and falls back for those dabs.
//
// One dispatch stamps one dab over one rectangle (the correctness kernel; batching many dabs into
// one dispatch is the next step, the maths here is what it will call).

struct Stamp {
    // x0, y0, width, height (pixel coordinates may be negative: the layer is an infinite plane).
    rect: vec4<u32>,
    // centre x, centre y (already snapped for an aliased dab), proj_scale, proj_angle.
    center: vec4<f32>,
    // radius, angle (radians; clockwise on screen when `c3`), roundness, alpha.
    shape: vec4<f32>,
    // hardness, flip_x (-1 or 1), flip_y (-1 or 1), unused.
    extra: vec4<f32>,
    // c3_tip_math, aliased, square (Block), triangle.
    flags: vec4<u32>,
    // wet_edges (single-dab Wet Edges), write_depth, small (area-sampled tip), unused.
    flags2: vec4<u32>,
    // Dab-invariant geometry from the CPU: roundness, scaled radius, reach, reach².
    geom: vec4<f32>,
    // The small tip's clipped reach², unused.
    geom2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> p: Stamp;
@group(0) @binding(1) var<storage, read_write> coverage: array<f32>;
@group(0) @binding(2) var<storage, read_write> depth: array<f32>;

// The CPU's 4×4 sub-pixel grid for the area coverage of a small tip.
const SUBPIXEL = array<f32, 4>(-0.375, -0.125, 0.125, 0.375);
const TIP_EDGE_PX: f32 = 1.2;

// Photoshop's measured round tip: solid core `r0 · R`, Gaussian shoulder `s · R` (see `lib.rs`).
fn tip_shape(radius_in: f32, hardness: f32) -> vec2<f32> {
    // A function-scope array so it can be indexed at run time.
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
    let s = a.y + (b.y - a.y) * t;
    let radius = max(radius_in, 0.0);
    // A tip of a few pixels keeps a narrower edge; never sharper than a quarter pixel.
    let edge = max(min(TIP_EDGE_PX, 0.3 * radius), 0.25);
    let core = max(min(r0 * radius, radius - edge), 0.0);
    let shoulder = max(max(s * radius, edge), 1e-3);
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
    // Keep the division: multiplying by a reciprocal changes coverage bits.
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

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let w = p.rect.z;
    let h = p.rect.w;
    if (gid.x >= w || gid.y >= h) {
        return;
    }
    let idx = gid.y * w + gid.x;
    coverage[idx] = 0.0;
    depth[idx] = 0.0;

    let cx = p.center.x;
    let cy = p.center.y;
    let r = p.shape.x;
    let angle = p.shape.y;
    let alpha = p.shape.w;
    let hardness = p.extra.x;
    let fx = p.extra.y;
    let fy = p.extra.z;
    let c3 = p.flags.x != 0u;
    let aliased = p.flags.y != 0u;
    let square = p.flags.z != 0u;
    let triangle = p.flags.w != 0u;
    let wet = p.flags2.x != 0u;

    // Dab-invariant geometry: the CPU computes these (`BrushStamp`) so the two paths share the
    // same rounding at the reach boundary, where one f32 ulp decides whether a rim pixel paints.
    let ro = p.geom.x;
    let rm = p.geom.y;
    let reach2 = p.geom.w;
    let small_reach2 = p.geom2.x;
    let small = p.flags2.z != 0u;

    // Angles turn clockwise on the screen under C3, counter-clockwise otherwise.
    let signed_angle = select(angle, -angle, c3);
    let sn = sin(signed_angle);
    let cs = cos(signed_angle);
    // Brush Projection: stretch sampling along the tilt direction, foreshortening the tip.
    let proj_scale = p.center.z;
    let proj_angle = p.center.w;
    let k = select(0.0, 1.0 / max(proj_scale, 0.05) - 1.0, proj_scale < 0.999);
    let ax = cos(proj_angle);
    let ay = sin(proj_angle);

    let x = i32(p.rect.x) + i32(gid.x);
    let y = i32(p.rect.y) + i32(gid.y);
    let dx = f32(x) + 0.5 - cx;
    let dy = f32(y) + 0.5 - cy;
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
            return;
        }
        val = 1.0;
        rn = 0.0;
    } else if (small) {
        let ru = u * ro;
        let d2 = ru * ru + v * v;
        if (d2 >= small_reach2) {
            return;
        }
        var sum = 0.0;
        for (var sy = 0u; sy < 4u; sy = sy + 1u) {
            for (var sx = 0u; sx < 4u; sx = sx + 1u) {
                let st = to_tip(dx + SUBPIXEL[sx], dy + SUBPIXEL[sy], ax, ay, k, sn, cs, fx, fy);
                let sr = st.x * ro;
                let sd = sqrt(sr * sr + st.y * st.y);
                // Each sample takes the tip's own edge, so the covered area is smooth.
                sum = sum + select(tip_falloff(sd, rm, hardness), c3_tip_falloff(sd / ro, r, hardness, true), c3);
            }
        }
        val = sum / 16.0;
        rn = sqrt(d2) / rm;
    } else {
        let ru = u * ro;
        let d2 = ru * ru + v * v;
        if (d2 >= reach2) {
            return;
        }
        let dd = sqrt(d2);
        val = tip_coverage(c3, aliased, dd, rm, ro, r, hardness);
        rn = dd / rm;
    }

    if (val <= 0.0) {
        return;
    }
    if (wet) {
        val = val * wet_factor(rn);
    }
    coverage[idx] = val * alpha;
    if (p.flags2.y != 0u) {
        depth[idx] = clamp(1.0 - rn, 0.0, 1.0);
    }
}
