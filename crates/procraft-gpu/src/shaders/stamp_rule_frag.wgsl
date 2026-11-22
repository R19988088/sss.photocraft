// Stamps laid by their stroke's rule in a fragment pass (see stamp_rule.wgsl), for a GPU without
// compute passes or half floats to draw into. One that can't draw into half floats lays every
// stroke so (`EIGHT`): a stroke there is kept twice, in 8 bits a channel (what everything else
// reads) and in half floats packed into integers (what it builds up in, so faint stamps add up as
// they would in half floats). Each fragment is a pixel of the part, written to both; the part is
// copied into the stroke after.

// the stroke is in 8 bits a channel, its half floats beside it (else it is in half floats itself)
override EIGHT: bool = true;

// the stamps, 7 vectors each (as RULE_STAMP_FLOATS floats)
struct RuleStamps {
    v: array<vec4<f32>, 1008>,
}

@group(0) @binding(1) var<uniform> rule_stamps: RuleStamps;
// the part's corner, tiles across and rule, then each tile's (first, count) into the stamp list
// after them, RULE_ROW words a row
@group(0) @binding(2) var rule_tiles: texture_2d<u32>;
// the stroke as it stands before these stamps, in 8 bits and in half floats (red and green, blue
// and alpha, each pair packed)
@group(0) @binding(3) var rule_before8: texture_2d<f32>;
@group(0) @binding(4) var rule_before: texture_2d<u32>;

fn rule_float(b: u32) -> f32 {
    return rule_stamps.v[b >> 2u][b & 3u];
}

fn rule_word(i: u32) -> u32 {
    return textureLoad(rule_tiles, vec2<i32>(i32(i % RULE_ROW), i32(i / RULE_ROW)), 0).r;
}

@vertex
fn vs_rule(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

struct RuleOut {
    @location(0) eight: vec4<f32>,
    @location(1) halves: vec2<u32>,
}

// `c` as the nearest 8-bit value each channel
fn to_eight(c: vec4<f32>) -> vec4<f32> {
    return floor(clamp(c, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0 + 0.5) / 255.0;
}

@fragment
fn fs_stamp_rule(@builtin(position) pos: vec4<f32>) -> RuleOut {
    let corner = vec2<i32>(bitcast<i32>(rule_word(0u)), bitcast<i32>(rule_word(1u)));
    let at = vec2<u32>(pos.xy);
    let px = corner + vec2<i32>(at);
    // the stroke as it was built up, unless something else drew into it since (then as it shows)
    let shown = textureLoad(rule_before8, px, 0);
    var start = shown;
    if (EIGHT) {
        let kept = textureLoad(rule_before, px, 0).rg;
        let built = vec4<f32>(unpack2x16float(kept.x), unpack2x16float(kept.y));
        let same = all(abs(to_eight(built) - shown) < vec4<f32>(0.5 / 255.0));
        start = select(shown, built, same);
    }
    // (rounded to half floats here, as they are kept)
    let paint = halves(rule_laid(start, px, at));
    var out: RuleOut;
    out.halves = vec2<u32>(pack2x16float(paint.rg), pack2x16float(paint.ba));
    out.eight = paint;
    if (EIGHT) {
        out.eight = to_eight(paint);
    }
    return out;
}
