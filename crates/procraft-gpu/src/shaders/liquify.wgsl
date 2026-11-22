// Liquify. The picture shows through a field of offsets (canvas pixels: each pixel shows the copy
// from that far away). The brush works in steps: a step's stamps add up what they do at each
// pixel, then the field is carried along by that, so what was moved before moves on with the rest
// and the picture behaves like something soft, however often the brush goes over it.

struct LiquifyUniform {
    // a stamp's (centre x, y, radius, amount); a step's (how much of the field stays, ...)
    stamp: vec4<f32>,
    // a stamp's (way x, y, mode, how narrow a lobe is); a step's (.., .., .., selected only)
    way: vec4<f32>,
    // (canvas width, height, field pixels a canvas pixel across, down)
    canvas: vec4<f32>,
    // the step's part of the field (x, y, width, height, field pixels)
    area: vec4<f32>,
}

@group(0) @binding(0) var<uniform> lu: LiquifyUniform;
// what the stamps add up to: (offset x, y, how much is rebuilt)
@group(1) @binding(0) var sums: texture_2d<f32>;
@group(1) @binding(1) var field: texture_2d<f32>;
@group(1) @binding(2) var chosen: texture_2d<f32>;

// how far a stamp moves things: what is behind a push comes on by this part of the brush's move;
// a twirl turns at this many radians a second, a pinch draws in by this part of the distance
const PUSH: f32 = 0.75;
const TURN: f32 = 4.0;
const DRAW: f32 = 1.5;
// rebuilding: a second of it at the centre leaves this, to this power, of the offsets; those under
// the least offset (pixels) go altogether
const REBUILD_KEEP: f32 = 0.75;
const REBUILD_POWER: f32 = 25.0;
const LEAST_OFFSET: f32 = 0.25;
const NEAREST: f32 = 0.3;

struct StampOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) local: vec2<f32>,
}

// a square around the stamp, in the step's part of the field
@vertex
fn vs_stamp(@builtin(vertex_index) vi: u32) -> StampOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let q = corners[vi % 6u];
    let p = (lu.stamp.xy + q * lu.stamp.z) * lu.canvas.zw - lu.area.xy;
    var out: StampOut;
    out.pos = vec4<f32>(p.x / lu.area.z * 2.0 - 1.0, 1.0 - p.y / lu.area.w * 2.0, 0.0, 1.0);
    out.local = q * lu.stamp.z;
    return out;
}

// all of it at the centre, none at the rim, half at half way, flat at both ends
fn falloff(d: f32) -> f32 {
    let x = clamp(d, 0.0, 1.0);
    if (x < 0.5) {
        return 1.0 - 2.0 * x * x;
    }
    return 2.0 * (1.0 - x) * (1.0 - x);
}

// What a stamp (`r` its radius, `amount` how much, `way` its way, mode and how narrow a lobe is)
// does `away` from its centre (canvas pixels), within its rim, the field `field_px` field pixels a
// canvas pixel: how far it moves things, and how much it rebuilds.
fn stamp_does(away: vec2<f32>, r: f32, amount: f32, way_mode: vec4<f32>, field_px: vec2<f32>) -> vec3<f32> {
    let dist = length(away);
    // no nearer than a part of a pixel of the field: a tiny stamp only just touches it
    let f = falloff(max(dist, NEAREST / max(field_px.x, field_px.y)) / r) * amount;
    let way = way_mode.xy;
    var moved = vec2<f32>(0.0);
    var rebuilt = 0.0;
    switch i32(way_mode.z + 0.5) {
        // push: what is behind the brush comes on
        case 0: { moved = way * (-PUSH * f); }
        // twirl right and left: turned about the centre
        case 1: { moved = vec2<f32>(away.y, -away.x) * (TURN * f); }
        case 2: { moved = vec2<f32>(-away.y, away.x) * (TURN * f); }
        // pinch and expand: drawn in, or out, a lobe only towards its way
        case 3: { moved = away * (DRAW * f); }
        case 4: { moved = away * (-DRAW * f); }
        case 5: {
            let narrow = way_mode.w;
            let towards = dot(away / max(dist, 1e-4), way);
            moved = away * (DRAW * f * clamp((towards - narrow) / (1.0 - narrow), 0.0, 1.0));
        }
        // edge: drawn in towards the brush's line, more the further from it
        case 6: {
            let across = vec2<f32>(-way.y, way.x);
            let side = dot(across, away);
            moved = across * (sign(side) * 0.5 * abs(side) * f / r);
        }
        // reconstruct
        default: { rebuilt = f; }
    }
    return vec3<f32>(moved, rebuilt);
}

// what one stamp does at its own pixel
@fragment
fn fs_stamp(in: StampOut) -> @location(0) vec4<f32> {
    if (length(in.local) >= lu.stamp.z) {
        discard;
    }
    return vec4<f32>(stamp_does(in.local, lu.stamp.z, lu.stamp.w, lu.way, lu.canvas.zw), 0.0);
}

@vertex
fn vs_step(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// the field at `p` (field pixels), between its pixels
fn field_at(p: vec2<f32>) -> vec2<f32> {
    let last = vec2<i32>(textureDimensions(field)) - 1;
    let at = p - 0.5;
    let below = floor(at);
    let t = at - below;
    let a = clamp(vec2<i32>(below), vec2<i32>(0), last);
    let b = clamp(vec2<i32>(below) + 1, vec2<i32>(0), last);
    let top = mix(textureLoad(field, a, 0).xy, textureLoad(field, vec2<i32>(b.x, a.y), 0).xy, t.x);
    let bottom = mix(textureLoad(field, vec2<i32>(a.x, b.y), 0).xy, textureLoad(field, b, 0).xy, t.x);
    return mix(top, bottom, t.y);
}

// how much of the field's pixel `here` a step takes: all, or as far as the selection does
fn taken_at(here: vec2<f32>) -> f32 {
    if (lu.way.w > 0.5) {
        return textureLoad(chosen, vec2<i32>(here / lu.canvas.zw), 0).r;
    }
    return 1.0;
}

// the offset a step leaves at a pixel its stamps added up `sum` at (taken that far), the field
// `carried` where they send it
fn stepped(sum: vec3<f32>, moved: vec2<f32>, carried: vec2<f32>, taken: f32) -> vec2<f32> {
    var offset = (moved + carried) * lu.stamp.x;
    if (sum.z > 0.0) {
        offset *= pow(REBUILD_KEEP, REBUILD_POWER * sum.z * taken);
        if (length(offset) < LEAST_OFFSET) {
            offset = vec2<f32>(0.0);
        }
    }
    return offset;
}

// the field after a step: where the stamps send this pixel, the offset from there on top
@fragment
fn fs_step(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let sum = textureLoad(sums, vec2<i32>(pos.xy), 0);
    let here = pos.xy + lu.area.xy;
    let taken = taken_at(here);
    let moved = sum.xy * taken;
    return vec4<f32>(stepped(sum.xyz, moved, field_at(here + moved * lu.canvas.zw), taken), 0.0, 1.0);
}

// ---- on a GPU that can't draw into floats: the stamps' sums and the field kept in integers
// holding each float's bits, the sums added up in a pass of their own a lot of stamps at a time,
// each stamp rounded into them as half floats are (as a blend into them adds it up)

struct LiquifyLot {
    // (stamps in the lot, 1 when the sums so far are read on from), the canvas and the step's part
    // of the field (as `lu` has them)
    head: vec4<f32>,
    canvas: vec4<f32>,
    area: vec4<f32>,
    // each stamp's (centre x, y, radius, amount) then (way x, y, mode, how narrow a lobe is)
    stamps: array<vec4<f32>, 512>,
}

@group(1) @binding(3) var<uniform> lot: LiquifyLot;
@group(1) @binding(4) var sums_bits: texture_2d<u32>;
@group(1) @binding(5) var field_bits: texture_2d<u32>;

// `x` rounded to the nearest half float (ties to even), kept in a float: worked out on its bits,
// so that no compiler takes a round trip through half floats for one that changes nothing
fn to_half(x: f32) -> f32 {
    if (abs(x) < 6.1035156e-5) {
        // below half floats' normal range: their step there
        return round(x * 16777216.0) / 16777216.0;
    }
    let b = bitcast<u32>(x);
    let kept = b + 4095u + ((b >> 13u) & 1u);
    return bitcast<f32>(kept & 0xffffe000u);
}

fn to_halves(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(to_half(v.x), to_half(v.y), to_half(v.z));
}

@fragment
fn fs_sums_bits(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<u32> {
    var sum = vec3<f32>(0.0);
    if (lot.head.y > 0.5) {
        sum = bitcast<vec4<f32>>(textureLoad(sums_bits, vec2<i32>(pos.xy), 0)).xyz;
    }
    // the pixel's middle on the canvas
    let at = (pos.xy + lot.area.xy) / lot.canvas.zw;
    for (var k = 0u; k < u32(lot.head.x); k++) {
        let s = lot.stamps[2u * k];
        let away = at - s.xy;
        if (length(away) >= s.z) {
            continue;
        }
        sum = to_halves(sum + to_halves(stamp_does(away, s.z, s.w, lot.stamps[2u * k + 1u], lot.canvas.zw)));
    }
    return bitcast<vec4<u32>>(vec4<f32>(sum, 0.0));
}

// the field (as bits) at `p` (field pixels), between its pixels
fn field_bits_at(p: vec2<f32>) -> vec2<f32> {
    let last = vec2<i32>(textureDimensions(field_bits)) - 1;
    let at = p - 0.5;
    let below = floor(at);
    let t = at - below;
    let a = clamp(vec2<i32>(below), vec2<i32>(0), last);
    let b = clamp(vec2<i32>(below) + 1, vec2<i32>(0), last);
    let top = mix(field_bits_texel(a), field_bits_texel(vec2<i32>(b.x, a.y)), t.x);
    let bottom = mix(field_bits_texel(vec2<i32>(a.x, b.y)), field_bits_texel(b), t.x);
    return mix(top, bottom, t.y);
}

fn field_bits_texel(q: vec2<i32>) -> vec2<f32> {
    return bitcast<vec2<f32>>(textureLoad(field_bits, q, 0).xy);
}

@fragment
fn fs_step_bits(@builtin(position) pos: vec4<f32>) -> @location(0) vec2<u32> {
    let sum = bitcast<vec4<f32>>(textureLoad(sums_bits, vec2<i32>(pos.xy), 0));
    let here = pos.xy + lu.area.xy;
    let taken = taken_at(here);
    let moved = sum.xy * taken;
    return bitcast<vec2<u32>>(stepped(sum.xyz, moved, field_bits_at(here + moved * lu.canvas.zw), taken));
}
