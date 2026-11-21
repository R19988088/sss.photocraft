// C3 colour transport (`transport.rs::feedback_dab`, V1) over one lane's pixels: the carrier
// builds from the layer under the dab, retains `keep` of the previous carrier, and is then laid
// back over the destination through the dab's coverage. All four slices are storage buffers,
// because `rgba32float` is not a guaranteed storage texture format and the CPU path indexes the
// pixels by hand anyway (bilinear reads with an explicit clamp, no samplers).

struct Params {
    // region: x0, y0, w, h — the document rectangle the pixel arrays cover.
    region: vec4<u32>,
    // rect: x0, y0, x1, y1 — the dab's rectangle, in document pixels.
    rect: vec4<u32>,
    // center: cx, cy, radius, side.
    center: vec4<f32>,
    // flow: keep, scale, strength, has_previous.
    flow: vec4<f32>,
    // misc: channels, alpha (0xFFFFFFFF for none), carrier_w, carrier_h.
    misc: vec4<u32>,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> layer_in: array<f32>;
@group(0) @binding(2) var<storage, read> carrier_in: array<f32>;
@group(0) @binding(3) var<storage, read_write> out_data: array<f32>;
@group(0) @binding(4) var<storage, read> coverage: array<f32>;

fn region_w() -> u32 { return p.region.z; }
fn region_h() -> u32 { return p.region.w; }

fn layer_at(x: u32, y: u32) -> vec4<f32> {
    let i = (y * region_w() + x) * 4u;
    return vec4<f32>(layer_in[i], layer_in[i + 1u], layer_in[i + 2u], layer_in[i + 3u]);
}

// `sample_into`: clamp to the region's own pixel centres, then bilinear between the four texels.
fn sample_layer(x: f32, y: f32) -> vec4<f32> {
    let lo = vec2<f32>(0.0, 0.0);
    let hi = vec2<f32>(f32(region_w()) - 1.0, f32(region_h()) - 1.0);
    let c = clamp(vec2<f32>(x, y), lo, hi);
    let base = vec2<i32>(i32(floor(c.x)), i32(floor(c.y)));
    let jx = min(u32(base.x) + 1u, region_w() - 1u);
    let jy = min(u32(base.y) + 1u, region_h() - 1u);
    let f = c - floor(c);
    let a = layer_at(u32(base.x), u32(base.y));
    let b = layer_at(jx, u32(base.y));
    let d = layer_at(u32(base.x), jy);
    let e = layer_at(jx, jy);
    let ab = a + (b - a) * f.x;
    let de = d + (e - d) * f.x;
    return ab * (1.0 - f.y) + de * f.y;
}

fn carrier_at(cw: u32, ch: u32, x: u32, y: u32) -> vec4<f32> {
    let i = (y * cw + x) * 4u;
    return vec4<f32>(carrier_in[i], carrier_in[i + 1u], carrier_in[i + 2u], carrier_in[i + 3u]);
}

fn sample_carrier(x: f32, y: f32) -> vec4<f32> {
    let cw = p.misc.z;
    let chh = p.misc.w;
    if (cw == 0u || chh == 0u) {
        return vec4<f32>(0.0);
    }
    let hi = vec2<f32>(f32(cw) - 1.0, f32(chh) - 1.0);
    let c = clamp(vec2<f32>(x, y), vec2<f32>(0.0), hi);
    let base = vec2<i32>(i32(floor(c.x)), i32(floor(c.y)));
    let jx = min(u32(base.x) + 1u, cw - 1u);
    let jy = min(u32(base.y) + 1u, chh - 1u);
    let f = c - floor(c);
    let a = carrier_at(cw, chh, u32(base.x), u32(base.y));
    let b = carrier_at(cw, chh, jx, u32(base.y));
    let d = carrier_at(cw, chh, u32(base.x), jy);
    let e = carrier_at(cw, chh, jx, jy);
    let ab = a + (b - a) * f.x;
    let de = d + (e - d) * f.x;
    return ab * (1.0 - f.y) + de * f.y;
}

fn mult_alpha(v: vec4<f32>) -> vec4<f32> {
    let ai = p.misc.y;
    if (ai >= 4u) {
        return v;
    }
    var o = v;
    let a = v[ai];
    for (var c = 0u; c < 4u; c = c + 1u) {
        if (c != ai) {
            o[c] = v[c] * a;
        }
    }
    return o;
}

fn with_alpha(v: vec4<f32>, a: f32) -> vec4<f32> {
    let ai = p.misc.y;
    if (ai >= 4u) {
        return v;
    }
    var o = v;
    o[ai] = a;
    return o;
}

// Build the carrier: `fresh` from the layer under the dab, `previous` from the last dab's carrier
// (scaled from its radius), blended by `keep`.
@compute @workgroup_size(8, 8)
fn build(@builtin(global_invocation_id) gid: vec3<u32>) {
    let side = u32(p.center.w);
    if (gid.x >= side || gid.y >= side) {
        return;
    }
    let half_side = f32(side) * 0.5;
    let dx = f32(gid.x) + 0.5 - half_side;
    let dy = f32(gid.y) + 0.5 - half_side;
    let sampled = mult_alpha(sample_layer(p.center.x + dx - 0.5, p.center.y + dy - 0.5));
    // The reference lays a second premultiply on top (`premultiplied_into`, then `fresh *= alpha`),
    // so a premultiplied alpha is squared — kept, because the carrier reads it back that way.
    var fresh = mult_alpha(sampled);
    var previous = vec4<f32>(0.0);
    var has_previous = false;
    if (p.flow.w > 0.5) {
        let scale = p.flow.y;
        let cw = f32(p.misc.z);
        let chh = f32(p.misc.w);
        previous = sample_carrier(dx * scale + cw * 0.5 - 0.5, dy * scale + chh * 0.5 - 0.5);
        let ai = p.misc.y;
        if (ai < 4u) {
            has_previous = previous[ai] > 0.0001;
        } else {
            has_previous = true;
        }
    }
    var pixel = fresh;
    if (has_previous) {
        pixel = fresh + (previous - fresh) * p.flow.x;
    } else {
        pixel = fresh;
    }
    let i = (gid.y * side + gid.x) * 4u;
    out_data[i] = pixel.x;
    out_data[i + 1u] = pixel.y;
    out_data[i + 2u] = pixel.z;
    out_data[i + 3u] = pixel.w;
}

// Lay the carrier over the layer through the dab's coverage (`over_carried`).
@compute @workgroup_size(8, 8)
fn lay(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = p.rect.x + gid.x;
    let oy = p.rect.y + gid.y;
    if (ox >= p.rect.z || oy >= p.rect.w) {
        return;
    }
    let lx = f32(ox) - f32(p.region.x);
    let ly = f32(oy) - f32(p.region.y);
    if (lx < 0.0 || ly < 0.0 || lx >= f32(region_w()) || ly >= f32(region_h())) {
        return;
    }
    let dest = sample_layer(lx, ly);
    let side = p.center.w;
    let local_x = f32(ox) + 0.5 - p.center.x + side * 0.5 - 0.5;
    let local_y = f32(oy) + 0.5 - p.center.y + side * 0.5 - 0.5;
    let carried = sample_carrier(local_x, local_y);
    let cw = u32(p.rect.z - p.rect.x);
    let chh = u32(p.rect.w - p.rect.y);
    let ci = gid.y * cw + gid.x;
    if (ci >= arrayLength(&coverage)) {
        return;
    }
    let coverage_value = clamp(coverage[ci] * p.flow.z, 0.0, 1.0);
    let ai = p.misc.y;
    var da = 1.0;
    var sa = coverage_value;
    if (ai < 4u) {
        da = dest[ai];
        sa = carried[ai] * coverage_value;
    }
    var result = dest;
    if (sa > 0.0) {
        let result_a = sa + da * (1.0 - sa);
        var mixed = dest;
        for (var c = 0u; c < 4u; c = c + 1u) {
            if (c != ai) {
                mixed[c] = (carried[c] * coverage_value + dest[c] * da * (1.0 - sa)) / max(result_a, 1e-6);
            }
        }
        if (ai < 4u) {
            mixed[ai] = result_a;
        }
        result = mixed;
    }
    let i = (u32(ly) * region_w() + u32(lx)) * 4u;
    out_data[i] = result.x;
    out_data[i + 1u] = result.y;
    out_data[i + 2u] = result.z;
    out_data[i + 3u] = result.w;
}
