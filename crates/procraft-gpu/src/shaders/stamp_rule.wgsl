// Stamps laid by their stroke's rule pixel by pixel, where drawing them "over" or keeping the
// strongest can't follow it: a stroke building up gradually (each stamp adds its cover of what
// is left, squared) and one building up towards its transfer, never past it and never back.
// Each pixel goes through the stamps over its tile in order, as its own paint stands after each,
// and is written once. Appended to stamp.wgsl, whose stamp_paint shades each stamp as the
// fragment shader does; then the pass that runs it (stamp_rule_compute.wgsl, or on a GPU without
// compute passes stamp_rule_frag.wgsl, which lays every stroke so), with where the stamps and the
// tiles are read from: `rule_float` and `rule_word`.

const RULE_TILE: u32 = 16u;
const RULE_ROW: u32 = 1024u;
// floats a stamp instance takes (see the canvas's StampInstance)
const RULE_STAMP_FLOATS: u32 = 28u;

// what the fragment shader is given for stamp `k` at canvas point `p`, if its quad reaches it
fn rule_stamp(k: u32, p: vec2<f32>, out: ptr<function, StampOut>) -> bool {
    let b = k * RULE_STAMP_FLOATS;
    let center = vec2<f32>(rule_float(b), rule_float(b + 1u));
    let radius = rule_float(b + 2u);
    let angle = rule_float(b + 4u);
    let roundness = rule_float(b + 5u);
    let r = max(radius, select(0.25, 0.01, su.shape >= 0));
    let half = vec2<f32>(r, max(r * roundness, 1e-3));
    let d = p - center;
    let c = cos(angle);
    let s = sin(angle);
    let q = vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c) / half;
    // the quad, a pixel to spare past the stamp
    let reach = (half + 1.0) / half;
    if (abs(q.x) > reach.x || abs(q.y) > reach.y) {
        return false;
    }
    let grain_offset = vec2<f32>(rule_float(b + 6u), rule_float(b + 7u));
    let flip = vec2<f32>(rule_float(b + 8u), rule_float(b + 9u));
    let tint = vec4<f32>(rule_float(b + 10u), rule_float(b + 11u), rule_float(b + 12u), rule_float(b + 13u));
    let look = vec4<f32>(rule_float(b + 14u), rule_float(b + 15u), rule_float(b + 16u), rule_float(b + 17u));
    let more = vec4<f32>(rule_float(b + 18u), rule_float(b + 19u), rule_float(b + 20u), rule_float(b + 21u));
    let turn = rule_float(b + 23u);
    let gc = cos(turn);
    let gs = sin(turn);
    (*out).pos = vec4<f32>(p, 0.0, 1.0);
    (*out).q = q;
    (*out).radius = r;
    (*out).alpha = rule_float(b + 3u);
    (*out).grain_at = grain_offset + vec2<f32>(d.x * gc - d.y * gs, d.x * gs + d.y * gc);
    (*out).fill = flip * half / (half + QUAD_REACH);
    (*out).color = mix(tinted(su.color.rgb, tint, rule_float(b + 22u)), su.secondary.rgb, look.z);
    (*out).look = look;
    (*out).grain_press = more.x;
    (*out).grain_bleed = more.y;
    (*out).transfer = more.z;
    (*out).layer = su.shape + i32(rule_float(b + 24u));
    (*out).finish = vec3<f32>(rule_float(b + 25u), rule_float(b + 26u), rule_float(b + 27u));
    return true;
}

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

// `c` rounded to half floats
fn halves(c: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(to_half(c.r), to_half(c.g), to_half(c.b), to_half(c.a));
}

// The paint at canvas pixel `px`, part pixel `at`, once the stamps over its tile are laid on
// `paint` by the part's rule (1: gradual, 2: towards the transfer; where every stroke is laid so,
// 0: over, 3: the strongest).
fn rule_laid(paint_in: vec4<f32>, px: vec2<i32>, at: vec2<u32>) -> vec4<f32> {
    let tile = at / RULE_TILE;
    let across = rule_word(2u);
    let rule = rule_word(3u);
    let entry = 4u + 2u * (tile.y * across + tile.x);
    let first = rule_word(entry);
    let count = rule_word(entry + 1u);
    var paint = paint_in;
    let p = vec2<f32>(px) + 0.5;
    var stamp: StampOut;
    for (var j = 0u; j < count; j++) {
        if (!rule_stamp(rule_word(first + j), p, &stamp)) {
            continue;
        }
        let laid = stamp_paint(stamp);
        if (rule == 3u) {
            // the strongest of each, as a blend by max keeps it
            paint = max(paint, laid);
            continue;
        }
        let cover = laid.a;
        if (cover <= 0.0) {
            continue;
        }
        if (rule == 0u) {
            // over, as a blend of premultiplied paint lays it into half floats, stamp by stamp (so
            // that a stroke comes out the same however its stamps are cut into passes)
            let src = halves(laid);
            paint = halves(src + paint * (1.0 - src.a));
            continue;
        }
        let a = clamp(paint.a, 0.0, 1.0);
        // how far towards the stamp's colour the pixel goes, drawn "over": what the rule adds,
        // over what was left
        var w = 0.0;
        if (rule == 1u) {
            // its cover of what is left, squared
            w = cover * (1.0 - a);
        } else {
            // towards its transfer by its cover, never past it, and never back from what is there
            let t = clamp(stamp.transfer, 0.0, 1.0);
            w = select(0.0, cover * (t - a) / max(1.0 - a, 1e-6), t > a);
        }
        w = clamp(w, 0.0, 1.0);
        paint = vec4<f32>(stamp.color * w, w) + paint * (1.0 - w);
    }
    return paint;
}
