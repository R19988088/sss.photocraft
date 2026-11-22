// Brush stamps into the stroke texture: one instanced quad per stamp, turned and squashed as the
// stamp says, with the round tip or a shape image, and a moving grain if the brush has one.
// Written premultiplied; the pipeline blends them "over" or keeps the maximum (see stamp_rule.wgsl
// for strokes that take their rule pixel by pixel).

// how far a stamp's quad reaches past the stamp each way (pixels): its shape fills all of it
const QUAD_REACH: f32 = 0.70710678;

struct StampUniform {
    canvas_size: vec2<f32>,
    hardness: f32,
    // -1: the round tip; otherwise a layer of `shapes`
    shape: i32,
    // straight sRGB colour
    color: vec4<f32>,
    // -1: no moving grain; otherwise a layer of `grains`
    grain: i32,
    grain_scale: f32,
    grain_depth: f32,
    grain_brightness: f32,
    grain_contrast: f32,
    // the grain turned, radians, and how it combines with the shape (a blend mode's number)
    grain_rotation: f32,
    grain_blend: u32,
    // 1: the grain read unfiltered (sharp)
    grain_sharp: u32,
    // the secondary colour (straight sRGB), mixed in as far as each stamp says
    secondary: vec4<f32>,
    // 1: the shape read unfiltered (sharp)
    shape_sharp: u32,
    // the coarsest mip level the shape is read from: a small stamp's thin shape keeps its strength
    shape_lod_cap: f32,
    // pixels a side of the image a moving grain is read from
    grain_side: f32,
    // what its shapes are (the canvas's TipKind): 0 masks, how much each pixel paints; 1 pictures
    // in their own colours; 2 lightness maps, their grey the colour's lightness; 3 gradient maps,
    // their grey a colour between the brush's and the secondary. All but masks RGBA, premultiplied.
    tip_kind: u32,
}

@group(0) @binding(0) var<uniform> su: StampUniform;
@group(1) @binding(0) var shapes: texture_2d_array<f32>;
@group(1) @binding(1) var grains: texture_2d_array<f32>;
@group(1) @binding(2) var shape_smp: sampler;
@group(1) @binding(3) var grain_smp: sampler;
// how much of each canvas pixel is selected (all of it without a selection)
@group(1) @binding(4) var selected: texture_2d<f32>;

struct StampIn {
    @location(0) center: vec2<f32>,
    @location(1) radius: f32,
    @location(2) alpha: f32,
    @location(3) angle: f32,
    @location(4) roundness: f32,
    @location(5) grain_offset: vec2<f32>,
    @location(6) flip: vec2<f32>,
    // the tint: the hue's turn, the saturation's keep and add, the brightness's keep
    @location(7) tint: vec4<f32>,
    // a moving grain's depth and tile size as multiples of the brush's; the secondary colour's share
    @location(8) look: vec4<f32>,
    // how far the paint fills a moving grain in, and room for more
    @location(9) more: vec4<f32>,
    // the tint's brightness add
    @location(10) brightness_add: f32,
    // how far the grain's patch under the stamp turns, where it is read from staying put
    @location(11) grain_turn: f32,
    // which shape image it is, counted on from the stroke's; how strongly a lightness map shows;
    // its sharpness threshold and how much below it stays as it is
    @location(12) tip: vec4<f32>,
}

struct StampOut {
    @builtin(position) pos: vec4<f32>,
    // position in the stamp's own frame, in radii (the shape spans -1..1)
    @location(0) q: vec2<f32>,
    @location(1) radius: f32,
    @location(2) alpha: f32,
    // where a moving grain is read from: the stamp's middle in it, plus the point's place from
    // there on the canvas, turned as far as the stamp says
    @location(3) grain_at: vec2<f32>,
    // how much of its quad the stamp itself is across each way, mirrored as the stamp says
    @location(4) fill: vec2<f32>,
    // the stamp's colour: the brush's tinted, mixed with the secondary
    @location(5) color: vec3<f32>,
    // how hard the paint is pressed into a moving grain, and how much that counts
    @location(8) grain_press: f32,
    @location(10) grain_bleed: f32,
    @location(6) look: vec4<f32>,
    // what light pressure lets through: the most a capped stroke builds up to
    @location(7) transfer: f32,
    // the layer of `shapes` it is drawn with
    @location(9) @interpolate(flat) layer: i32,
    // how strongly a lightness map shows, and the sharpness threshold and its soft share
    @location(11) @interpolate(flat) finish: vec3<f32>,
}

@vertex
fn vs_stamp(@builtin(vertex_index) vi: u32, inst: StampIn) -> StampOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    // a shape's quad keeps its reach however small the stamp; the round tip thins out by its area
    let r = max(inst.radius, select(0.25, 0.01, su.shape >= 0));
    let half = vec2<f32>(r, max(r * inst.roundness, 1e-3));
    // one pixel to spare for anti-aliasing
    let q = corners[vi % 6u] * (half + 1.0) / half;
    let local = q * half;
    let c = cos(inst.angle);
    let s = sin(inst.angle);
    let p = inst.center + vec2<f32>(local.x * c - local.y * s, local.x * s + local.y * c);
    var o: StampOut;
    o.pos = vec4<f32>(p.x / su.canvas_size.x * 2.0 - 1.0, 1.0 - p.y / su.canvas_size.y * 2.0, 0.0, 1.0);
    o.q = q;
    o.radius = r;
    o.alpha = inst.alpha;
    let gc = cos(inst.grain_turn);
    let gs = sin(inst.grain_turn);
    let from_middle = p - inst.center;
    o.grain_at = inst.grain_offset + vec2<f32>(from_middle.x * gc - from_middle.y * gs, from_middle.x * gs + from_middle.y * gc);
    o.fill = inst.flip * half / (half + QUAD_REACH);
    o.color = mix(tinted(su.color.rgb, inst.tint, inst.brightness_add), su.secondary.rgb, inst.look.z);
    o.look = inst.look;
    o.grain_press = inst.more.x;
    o.grain_bleed = inst.more.y;
    o.transfer = inst.more.z;
    o.layer = su.shape + i32(inst.tip.x);
    o.finish = inst.tip.yzw;
    return o;
}

// HSL lightness, as an image editor's lightness blending reckons it
fn hsl_lightness(c: vec3<f32>) -> f32 {
    return 0.5 * (max(c.r, max(c.g, c.b)) + min(c.r, min(c.g, c.b)));
}

// `c` moved to lightness `l`, its hue and saturation kept as far as the colour gamut lets
fn with_lightness(c: vec3<f32>, l: f32) -> vec3<f32> {
    let moved = c + (l - hsl_lightness(c));
    let m = hsl_lightness(moved);
    let lo = min(moved.r, min(moved.g, moved.b));
    let hi = max(moved.r, max(moved.g, moved.b));
    var out = moved;
    if (lo < 0.0) {
        out = m + (out - m) * m / max(m - lo, 1e-6);
    }
    if (hi > 1.0) {
        out = m + (out - m) * (1.0 - m) / max(hi - m, 1e-6);
    }
    return clamp(out, vec3<f32>(0.0), vec3<f32>(1.0));
}

// textures that colour a stamp (see blend.wgsl's texture rules): by their grey, its lightness, or
// a colour along the gradient from the colour to the secondary
const TEXTURE_LIGHTNESS: u32 = 46u;
const TEXTURE_GRADIENT: u32 = 47u;

// `colour` at the lightness grey `g` says (a middle grey keeping its own), as strongly as
// `strength` goes: the same curve through the colour's own lightness a lightness tip takes
fn lightness_from(colour: vec3<f32>, g: f32, strength: f32) -> vec3<f32> {
    let m = (g - 0.5) * strength + 0.5;
    let b = 4.0 * hsl_lightness(colour) - 1.0;
    return with_lightness(colour, clamp((1.0 - b) * m * m + b * m, 0.0, 1.0));
}

// the colour a picture-like tip's texel `t` (premultiplied) gives a stamp of `colour`, by the
// stroke's tip kind: its own; the colour at the lightness its grey says (a middle grey keeping the
// colour's own, the stronger the further from it); or between the colour and the secondary
fn tip_colour(colour: vec3<f32>, t: vec4<f32>, strength: f32) -> vec3<f32> {
    let straight = t.rgb / max(t.a, 1e-5);
    if (su.tip_kind == 1u) {
        return straight;
    }
    if (su.tip_kind == 2u) {
        return lightness_from(colour, straight.r, strength);
    }
    return mix(colour, su.secondary.rgb, straight.r);
}

// hue (0..1), saturation and brightness of a colour, and back
fn to_hsb(c: vec3<f32>) -> vec3<f32> {
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    let d = hi - lo;
    if (d <= 1e-5) {
        return vec3<f32>(0.0, 0.0, hi);
    }
    var h = 0.0;
    if (hi == c.r) {
        h = (c.g - c.b) / d;
    } else if (hi == c.g) {
        h = (c.b - c.r) / d + 2.0;
    } else {
        h = (c.r - c.g) / d + 4.0;
    }
    return vec3<f32>(fract(h / 6.0), d / hi, hi);
}

fn from_hsb(c: vec3<f32>) -> vec3<f32> {
    let k = fract(vec3<f32>(c.x) + vec3<f32>(5.0, 3.0, 1.0) / 6.0) * 6.0;
    return c.z - c.z * c.y * clamp(min(k, 4.0 - k), vec3<f32>(0.0), vec3<f32>(1.0));
}

// `c` tinted: its hue turned, its saturation and brightness each kept by so much and added to
fn tinted(c: vec3<f32>, t: vec4<f32>, brightness_add: f32) -> vec3<f32> {
    if (all(t == vec4<f32>(0.0, 1.0, 0.0, 1.0)) && brightness_add == 0.0) {
        return c;
    }
    let hsb = to_hsb(c);
    let s = clamp(hsb.y * t.y + t.z, 0.0, 1.0);
    let b = clamp(hsb.z * t.w + brightness_add, 0.0, 1.0);
    return from_hsb(vec3<f32>(fract(hsb.x + t.x), s, b));
}

// the shape image's texel under `uv`, unfiltered
fn shape_texel(uv: vec2<f32>, layer: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(shapes));
    let at = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - 1);
    return textureLoad(shapes, at, layer, 0);
}

@fragment
fn fs_stamp(i: StampOut) -> @location(0) vec4<f32> {
    return stamp_paint(i);
}

// a stamp's premultiplied paint at a pixel
fn stamp_paint(i: StampOut) -> vec4<f32> {
    let r = i.radius;
    var a = 0.0;
    var colour = i.color;
    if (su.shape < 0) {
        let d = length(i.q) * r;
        // a one-pixel anti-aliased rim, and the brush's own falloff from hardness·r outwards
        let rim = clamp(r - d + 0.5, 0.0, 1.0);
        let soft = 1.0 - smoothstep(min(su.hardness, 0.999) * r, r + 0.5, d);
        a = min(rim, soft);
    } else {
        // the shape fills the quad it is drawn on, which shows on a stamp a few pixels across
        let uv = i.q * i.fill * 0.5 + 0.5;
        // the mip level that matches the quad's size on the canvas
        // (a brush file's own image is bigger than the brush images, by as many levels)
        let side = f32(textureDimensions(shapes).x);
        let lod = clamp(log2(0.5 * side / (r + QUAD_REACH)), 0.0, su.shape_lod_cap + log2(side / 256.0));
        let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
        var t = textureSampleLevel(shapes, shape_smp, uv, i.layer, lod);
        if (su.shape_sharp == 1u) {
            t = shape_texel(uv, i.layer);
        }
        var s = t.r;
        if (su.tip_kind > 0u) {
            // a picture-like tip: as far as it is opaque, in the colour it gives
            s = t.a;
            colour = tip_colour(colour, t, i.finish.x);
        }
        a = select(0.0, s, inside);
    }
    // made sharp: what is over the threshold paints fully, what is well under it not at all
    if (i.finish.y > 0.0) {
        let tolerance = 1.0 - i.finish.y;
        if (a > tolerance) {
            a = 1.0;
        } else if (a <= (1.0 - i.finish.z) * tolerance) {
            a = 0.0;
        }
    }
    if (su.grain >= 0) {
        let scale = su.grain_scale * i.look.y;
        let gc = cos(su.grain_rotation);
        let gs = sin(su.grain_rotation);
        let at = i.grain_at;
        let g_uv = vec2<f32>(at.x * gc - at.y * gs, at.x * gs + at.y * gc) / scale;
        let sharp = su.grain_sharp == 1u;
        // a brush file's own image has the fine detail itself; ours only stands in for it
        let own = su.grain_side > 256.0;
        let g_lod = max(log2(su.grain_side / scale), 0.0);
        var raw = textureSampleLevel(grains, grain_smp, g_uv, su.grain, select(g_lod, 0.0, sharp && !own)).r;
        if (sharp) {
            if (own) {
                // unfiltered: the texel under it at its level
                let l = i32(round(g_lod));
                raw = textureLoad(grains, vec2<i32>(fract(g_uv) * vec2<f32>(textureDimensions(grains, l))), su.grain, l).r;
            } else {
                raw = sharp_grain(raw, g_uv);
            }
        }
        let g = clamp((raw - 0.5) * (1.0 + su.grain_contrast) + 0.5 + su.grain_brightness, 0.0, 1.0);
        let depth = clamp(su.grain_depth * i.look.x, 0.0, 1.0);
        if (su.grain_blend == TEXTURE_LIGHTNESS) {
            // the texture's grey the colour's lightness, as strongly as the stamp says
            colour = lightness_from(colour, g, depth);
        } else if (su.grain_blend == TEXTURE_GRADIENT) {
            // a colour between the colour and the secondary by the texture's grey, mixed in as far
            colour = mix(colour, mix(su.color.rgb, su.secondary.rgb, g), depth);
        } else {
            // combined with the shape by the grain's blend mode, as deep as it says, inside the shape
            a = grain_combine(a, g, depth, su.grain_blend);
        }
    }
    // a leaning pen shades it down its shape's height: from the gradation's share less at the
    // top of the shape to all of it at the bottom, cubed (so the top fades fast), before the bleed
    // takes its contrast
    if (i.look.w > 0.0) {
        let down = clamp(i.q.y * i.fill.y * 0.5 + 0.5, 0.0, 1.0);
        let g = 1.0 - i.look.w * (1.0 - down);
        a = a * g * g * g;
    }
    // the pressure's bleed, a contrast on the shape and its grain: none of the faint parts, all
    // of the strong ones, and more of both the harder the pen presses
    if (i.grain_bleed > 0.0) {
        let b = 8.0 * i.grain_bleed;
        a = clamp((1.0 + b) * a - b / sqrt(max(b * i.grain_press, 1e-4)), 0.0, 1.0);
    }
    // the round tip's rim takes a pixel as covered to its middle: a tip under a pixel across
    // covers only its area of one (a shape's quad reaches past it, which thins it as far)
    if (su.shape < 0) {
        let small = min(r * 2.0, 1.0);
        a = a * small * small;
    }
    // paint keeps inside the selection
    let inside_selection = textureSampleLevel(selected, shape_smp, i.pos.xy / su.canvas_size, 0.0).r;
    a = a * i.alpha * inside_selection;
    return vec4<f32>(colour * a, a);
}
