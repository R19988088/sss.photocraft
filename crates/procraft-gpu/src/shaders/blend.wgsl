// Blend functions on straight colour, shared by the shaders that blend (prepended to them):
// the separable and non-separable modes as usually defined for compositing, numbered as the
// layers' blend modes are.

// How much of a grain the paint takes when pressed into it at `press` (0..1): what stands above
// a level that sinks as the pen presses harder, nothing at no pressure and all of it at full,
// the grain's highest parts first.
const PRESS_SOFT: f32 = 0.08;
fn pressed_in(grain: f32, press: f32) -> f32 {
    let level = mix(1.0 + PRESS_SOFT, PRESS_SOFT, clamp(press, 0.0, 1.0));
    return smoothstep(level - PRESS_SOFT, level + PRESS_SOFT, grain);
}

// Wet edges: paint showing `c` (0..1) pools to 1 - |sin(x) / x| at x = 4.5c, which rises to full
// where it shows about 0.7 (a rim where it thins out) and settles back to about 0.78 where it is
// full; faint paint fades. This is that over `c`, the scale the paint's coverage takes. For thin
// paint it is the series of 1 - sin(x) / x: worked out directly, the subtraction leaves only the
// rounding of sin (all a GPU's sin keeps of so small a turn), which the division by `c` blows up
// into paint far past full.
fn pooled_over(c: f32) -> f32 {
    let x = 4.5 * max(c, 0.0);
    if (x < 0.5) {
        let x2 = x * x;
        return 4.5 * x * (1.0 / 6.0 - x2 / 120.0 + x2 * x2 / 5040.0);
    }
    return (1.0 - abs(sin(x) / x)) / max(c, 1e-6);
}

// a grain read unfiltered: the grain's own images are a quarter of the size of what they stand
// for, so the finer texels a pixel would land on (four to each of ours, tied to the grain so they
// move with it) are made up around the value read
fn sharp_grain(v: f32, uv: vec2<f32>) -> f32 {
    let cell = floor(uv * 1024.0);
    let n = fract(sin(dot(cell, vec2<f32>(12.9898, 78.233))) * 43758.547);
    return clamp(v + (n - 0.5) * 0.22, 0.0, 1.0);
}

fn blend_rgb(cb: vec3<f32>, cs: vec3<f32>, mode: u32) -> vec3<f32> {
    switch (mode) {
        case 1u: { return cb * cs; }
        case 2u: { return min(cb, cs); }
        case 3u: { return vec3<f32>(burn(cb.x, cs.x), burn(cb.y, cs.y), burn(cb.z, cs.z)); }
        case 4u: { return max(cb + cs - 1.0, vec3<f32>(0.0)); }
        case 5u: { return select(cb, cs, lum(cs) < lum(cb)); }
        case 6u: { return max(cb, cs); }
        case 7u: { return cb + cs - cb * cs; }
        case 8u: { return vec3<f32>(dodge(cb.x, cs.x), dodge(cb.y, cs.y), dodge(cb.z, cs.z)); }
        case 9u: { return min(cb + cs, vec3<f32>(1.0)); }
        case 10u: { return select(cb, cs, lum(cs) > lum(cb)); }
        case 11u: { return vec3<f32>(hard(cs.x, cb.x), hard(cs.y, cb.y), hard(cs.z, cb.z)); }
        case 12u: { return vec3<f32>(soft(cb.x, cs.x), soft(cb.y, cs.y), soft(cb.z, cs.z)); }
        case 13u: { return vec3<f32>(hard(cb.x, cs.x), hard(cb.y, cs.y), hard(cb.z, cs.z)); }
        case 14u: { return vec3<f32>(vivid(cb.x, cs.x), vivid(cb.y, cs.y), vivid(cb.z, cs.z)); }
        case 15u: { return clamp(cb + 2.0 * cs - 1.0, vec3<f32>(0.0), vec3<f32>(1.0)); }
        case 16u: { return vec3<f32>(pin(cb.x, cs.x), pin(cb.y, cs.y), pin(cb.z, cs.z)); }
        case 17u: { return select(vec3<f32>(0.0), vec3<f32>(1.0), cb + cs >= vec3<f32>(1.0)); }
        case 18u: { return abs(cb - cs); }
        case 19u: { return cb + cs - 2.0 * cb * cs; }
        case 20u: { return max(cb - cs, vec3<f32>(0.0)); }
        case 21u: { return vec3<f32>(divide(cb.x, cs.x), divide(cb.y, cs.y), divide(cb.z, cs.z)); }
        case 22u: { return set_lum(set_sat(cs, sat(cb)), lum(cb)); }
        case 23u: { return set_lum(set_sat(cb, sat(cs)), lum(cb)); }
        case 24u: { return set_lum(cs, lum(cb)); }
        case 25u: { return set_lum(cb, lum(cs)); }
        // shade, on straight colour (the layers' own is in `blend`)
        case 26u: { return min(cb, cs); }
        default: { return cs; }
    }
}

fn burn(b: f32, s: f32) -> f32 {
    if (b >= 1.0) { return 1.0; }
    if (s <= 0.0) { return 0.0; }
    return 1.0 - min(1.0, (1.0 - b) / s);
}

fn dodge(b: f32, s: f32) -> f32 {
    if (b <= 0.0) { return 0.0; }
    if (s >= 1.0) { return 1.0; }
    return min(1.0, b / (1.0 - s));
}

// hard light of `s` over `b` (overlay is hard light with the two swapped)
fn hard(b: f32, s: f32) -> f32 {
    if (s <= 0.5) { return b * 2.0 * s; }
    let t = 2.0 * s - 1.0;
    return b + t - b * t;
}

fn soft(b: f32, s: f32) -> f32 {
    if (s <= 0.5) {
        return b - (1.0 - 2.0 * s) * b * (1.0 - b);
    }
    var d = sqrt(b);
    if (b <= 0.25) {
        d = ((16.0 * b - 12.0) * b + 4.0) * b;
    }
    return b + (2.0 * s - 1.0) * (d - b);
}

fn vivid(b: f32, s: f32) -> f32 {
    if (s <= 0.5) { return burn(b, 2.0 * s); }
    return dodge(b, 2.0 * (s - 0.5));
}

fn pin(b: f32, s: f32) -> f32 {
    if (s <= 0.5) { return min(b, 2.0 * s); }
    return max(b, 2.0 * s - 1.0);
}

fn divide(b: f32, s: f32) -> f32 {
    if (s <= 0.0) { return select(0.0, 1.0, b > 0.0); }
    return min(1.0, b / s);
}

fn lum(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.3, 0.59, 0.11));
}

fn clip_color(c: vec3<f32>) -> vec3<f32> {
    let l = lum(c);
    let n = min(c.x, min(c.y, c.z));
    let x = max(c.x, max(c.y, c.z));
    var r = c;
    if (n < 0.0) {
        r = l + (r - l) * l / max(l - n, 1e-6);
    }
    if (x > 1.0) {
        r = l + (r - l) * (1.0 - l) / max(x - l, 1e-6);
    }
    return r;
}

fn set_lum(c: vec3<f32>, l: f32) -> vec3<f32> {
    return clip_color(c + (l - lum(c)));
}

fn sat(c: vec3<f32>) -> f32 {
    return max(c.x, max(c.y, c.z)) - min(c.x, min(c.y, c.z));
}

fn set_sat(c: vec3<f32>, s: f32) -> vec3<f32> {
    let n = min(c.x, min(c.y, c.z));
    let x = max(c.x, max(c.y, c.z));
    if (x <= n) {
        return vec3<f32>(0.0);
    }
    return (c - n) * s / (x - n);
}

// one value blended with another: a grey through the colour blend
fn blend_value(b: f32, s: f32, mode: u32) -> f32 {
    return blend_rgb(vec3<f32>(b), vec3<f32>(s), mode).x;
}

// a stroke's coverage `a` combined with another amount `b` (its grain, its second's coverage) by
// a blend mode: the modes that pick or carry over colour alone (darker and lighter colour, hue,
// saturation, colour, luminosity) leave the coverage as it is; soft light lifts or lowers it
// from a quarter of the way in, as far as `b` is past its middle; the rest as the colour blends
fn blend_amount(a: f32, b: f32, mode: u32) -> f32 {
    switch (mode) {
        case 5u, 10u, 22u, 23u, 24u, 25u: {
            return a;
        }
        case 12u: {
            let k = 0.5 * a + 0.25;
            return clamp(select(1.0 - 2.0 * (1.0 - b) * (1.0 - k), 2.0 * b * k, b < 0.5), 0.0, 1.0);
        }
        default: {
            return blend_value(a, b, mode);
        }
    }
}

// coverage `a` and grain `g` combined by the grain's blend mode `mode`, as deep as `depth`: two
// height-like modes of the grain's own (27: the coverage raised twelvefold, less the grain; 28:
// the same or as much again over the grain's light), normal laying the coverage over the grain,
// and every other mode by the blend mode's own rule
fn grain_combine(a: f32, g: f32, depth: f32, mode: u32) -> f32 {
    if (mode >= TEXTURE_RULES) {
        return texture_rule(a, g, depth, mode);
    }
    let h = 12.0 * a * depth;
    var r = 0.0;
    if (mode == 27u) {
        r = clamp(h - g, 0.0, 1.0);
    } else if (mode == 28u) {
        r = clamp(max(h * (1.0 - g), clamp(h - g, 0.0, 1.0)), 0.0, 1.0);
    } else {
        var blended = g + a * (1.0 - g);
        if (mode != 0u) {
            blended = clamp(blend_amount(a, g, mode), 0.0, 1.0);
        }
        r = mix(a, blended, depth);
    }
    // where the grain raises the coverage, faint coverage takes only part of the rise (all of it
    // from GRAIN_RISE_FULL up; none where there is none)
    if (r > a) {
        r = a + min(a / GRAIN_RISE_FULL, 1.0) * (r - a);
    }
    return r;
}

// The coverage from which a grain raises it fully (a sixty-fourth, as 1/63.75).
const GRAIN_RISE_FULL: f32 = 0.015686275;

// Texture rules (a plugin's brush engine names them for its dabs' texture): from 32, the texture's
// value `m` combined with a dab's coverage `a` at a strength `s` by one of fourteen ways (multiply,
// subtract, darken, overlay, dodge, burn, linear dodge, linear burn, hard mix, softer hard mix,
// height, linear height, and the last two weighed as image editors weigh them); from 48 the same,
// softly: the strength taking the texture away rather than the paint.
const TEXTURE_RULES: u32 = 32u;
const TEXTURE_SOFT: u32 = 48u;

fn union_of(a: f32, b: f32) -> f32 {
    return a + b - a * b;
}

fn dodge_alpha(src: f32, dst: f32) -> f32 {
    if (src >= 0.9999) {
        return select(1.0, 0.0, dst <= 0.0);
    }
    return clamp(dst / (1.0 - src), 0.0, 1.0);
}

fn burn_alpha(src: f32, dst: f32) -> f32 {
    if (dst >= 0.9999) {
        return 1.0;
    }
    if (src < 1.0 - dst) {
        return 0.0;
    }
    return clamp(1.0 - (1.0 - dst) / max(src, 1e-6), 0.0, 1.0);
}

fn overlay_alpha(src: f32, dst: f32) -> f32 {
    if (dst > 0.5) {
        return 1.0 - (1.0 - src) * (1.0 - (2.0 * dst - 1.0));
    }
    return 2.0 * src * dst;
}

fn texture_rule(a: f32, m: f32, strength: f32, mode: u32) -> f32 {
    let soft = mode >= TEXTURE_SOFT;
    let rule = select(mode - TEXTURE_RULES, mode - TEXTURE_SOFT, soft);
    let s = clamp(strength, 0.0, 1.0);
    // the texture kept where the strength falls short, or the paint taken down by it
    let ms = select(m, union_of(m, 1.0 - s), soft);
    let ad = select(a * s, a, soft);
    // the heights' strength, kept short of whole
    let h = 0.99 * s;
    let keep = 1.0 - h;
    var r = 0.0;
    switch (rule) {
        case 0u: { r = ms * ad; }
        case 1u: { r = select(a - (m + 1.0 - s), a - m * s, soft); }
        case 2u: { r = min(ms, ad); }
        case 3u: { r = overlay_alpha(ms, ad); }
        case 4u: { r = dodge_alpha(select(m, m * s, soft), select(a * s, a, soft)); }
        case 5u: { r = burn_alpha(ms, ad); }
        case 6u: { r = select(m + a * s, m * s + a, soft); }
        case 7u: { r = ms + ad - 1.0; }
        case 8u: {
            let mixed = select(0.0, 1.0, ms + ad > 1.0);
            r = select(mixed, mixed * union_of(a, s), soft);
        }
        case 9u: { r = 3.0 * ad - 2.0 * (1.0 - select(m, m * s, soft)); }
        case 10u: { r = select(a / keep - (m + keep), a / keep - m * h, soft); }
        case 11u: {
            let raised = select(a / keep - keep, a / keep, soft);
            let taken = select(m, m * h, soft);
            r = max(raised * (1.0 - taken), raised - taken);
        }
        case 12u: { r = select(a * 10.0 * s - m, a + a * 9.0 * s - m * s, soft); }
        case 13u: {
            let raised = select(a * 10.0 * s, a + a * 9.0 * s, soft);
            let taken = select(m, m * s, soft);
            r = max((1.0 - taken) * raised, raised - taken);
        }
        default: { r = a; }
    }
    return clamp(r, 0.0, 1.0);
}

// a dual brush's two strokes' own modes (the grain's numbers)
const DUAL_HEIGHT: u32 = 27u;
const DUAL_LINEAR_HEIGHT: u32 = 28u;

// the first stroke's amount `a` cut by the second's `b` as a height: where the second is thin
// (under 0.2) the first rises (most where it is near none or full), and the second's coverage
// lifts what the first has
fn dual_height(amount: f32, b: f32) -> f32 {
    // (held to 0..1: out of the grain it can be a rounding past 1, and then `mid` a hair below 0)
    let a = clamp(amount, 0.0, 1.0);
    let mid = 2.0 * (0.5 - abs(a - 0.5));
    let rise = 40.0 * max(0.2 - b, 0.0) * (sixth(a) + sixth(mid));
    return clamp(max(rise + 0.5 * a, (rise - a) * b + 2.0 * a), 0.0, 1.0);
}

// `x` to the sixth power, multiplied out: pow is undefined below 0 (NaN on most GPUs, which a
// stroke then draws as clear)
fn sixth(x: f32) -> f32 {
    let x2 = x * x;
    return x2 * x2 * x2;
}

// set in a dual mode (over its number + 1): the classic Normal combine, where the first's colour
// is kept whole beside the second's share of it
const DUAL_CLASSIC: u32 = 65536u;

// a dual mode as set (its number + 1, perhaps with DUAL_CLASSIC): its number
fn dual_mode(dual: u32) -> u32 {
    return (dual & 0xffffu) - 1u;
}

// a dual brush's first stroke's coverage `a` and its second's `b` combined by the brush's dual
// mode: Normal shows the second where the first isn't, the heights cut one by the other, the
// rest blend the two as the layers' modes do
fn dual_combine(a: f32, b: f32, mode: u32) -> f32 {
    if (mode == DUAL_HEIGHT || mode == DUAL_LINEAR_HEIGHT) {
        return dual_height(a, b);
    }
    if (mode != 0u) {
        return clamp(blend_amount(a, b, mode), 0.0, 1.0);
    }
    return a + (1.0 - a) * b;
}

// a dual brush's combined coverage `x` held to what the first stroke laid, `built`: as a grain
// raises it, faint coverage takes only part of the rise and none takes none, so a second adds
// nothing where the first isn't (but for Normal, which shows the second there)
fn dual_rise(x: f32, built: f32, mode: u32) -> f32 {
    if (mode == 0u || x <= built) {
        return x;
    }
    return built + min(built / GRAIN_RISE_FULL, 1.0) * (x - built);
}

fn unpremultiply(c: vec4<f32>) -> vec3<f32> {
    if (c.a <= 0.0) {
        return vec3<f32>(0.0);
    }
    return clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0));
}

// source `s` over backdrop `b` (both premultiplied) with blend function `mode`
fn blend(b: vec4<f32>, s: vec4<f32>, mode: u32) -> vec4<f32> {
    // shade: the darker of the backdrop and the source laid over it, premultiplied and alpha too
    if (mode == 26u) {
        return min(b, s + b * (1.0 - s.a));
    }
    // behind (a brush's): the source laid under the backdrop
    if (mode == 29u) {
        return b + s * (1.0 - b.a);
    }
    let cb = unpremultiply(b);
    let cs = unpremultiply(s);
    let mixed = clamp(blend_rgb(cb, cs, mode), vec3<f32>(0.0), vec3<f32>(1.0));
    let rgb = s.a * (1.0 - b.a) * cs + s.a * b.a * mixed + (1.0 - s.a) * b.rgb;
    return vec4<f32>(rgb, s.a + b.a * (1.0 - s.a));
}
