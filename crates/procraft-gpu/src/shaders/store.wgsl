// What a brush keeps with a layer between strokes (see canvas/store.rs): the paint it has laid there,
// its colours before they are lit, and a height, a grey as opaque as the paint is thick there. A
// stroke lays its paint into the first, each stamp's tip's grey over the second (fs_height), and
// the layer is drawn from the two (fs_lit): the paint at the lightness the height says.

// how far a stamp's quad reaches past the stamp each way (pixels)
const QUAD_REACH: f32 = 0.70710678;
// the grey a tip flattened all the way is (the middle of 255, rounded down)
const FLAT: f32 = 0.49803922;

@group(0) @binding(0) var shapes: texture_2d_array<f32>;
@group(0) @binding(2) var shape_smp: sampler;
// how much of each canvas pixel is selected (all of it without a selection)
@group(0) @binding(4) var selected: texture_2d<f32>;

struct HeightIn {
    // the stamp's middle, half its width and its squash
    @location(0) a: vec4<f32>,
    // its turn (radians), -1 where its tip is mirrored across and down (else 1), its tip's layer
    @location(1) b: vec4<f32>,
    // how opaque its grey goes on, how far its tip's grey keeps from a flat middle grey (0..1),
    // what its tips are (the canvas's TipKind) and 1 to read them unfiltered
    @location(2) c: vec4<f32>,
    // the canvas's width and height, and the coarsest mip level the tip is read from
    @location(3) d: vec4<f32>,
    // the pixels it can touch: x0, y0, x1, y1
    @location(4) e: vec4<f32>,
}

struct HeightOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) a: vec4<f32>,
    @location(1) @interpolate(flat) b: vec4<f32>,
    @location(2) @interpolate(flat) c: vec4<f32>,
    @location(3) @interpolate(flat) d: vec4<f32>,
}

// The stamp's box, as two triangles.
@vertex
fn vs_height(@builtin(vertex_index) vi: u32, inst: HeightIn) -> HeightOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vi % 6u];
    let p = mix(inst.e.xy, inst.e.zw, corner);
    var o: HeightOut;
    o.pos = vec4<f32>(p.x / inst.d.x * 2.0 - 1.0, 1.0 - p.y / inst.d.y * 2.0, 0.0, 1.0);
    o.a = inst.a;
    o.b = inst.b;
    o.c = inst.c;
    o.d = inst.d;
    return o;
}

// The stamp's tip's grey at the pixel, as opaque as the tip is there times how opaque it goes on,
// premultiplied, to lay over the height. The tip is read where a smudge's stamp reads it, pixel by
// pixel in its own frame, so the grey lies just where its paint went.
@fragment
fn fs_height(i: HeightOut) -> @location(0) vec4<f32> {
    let center = i.a.xy;
    let radius = max(i.a.z, 0.5);
    let roundness = i.a.w;
    let d = i.pos.xy - center;
    let c = cos(i.b.x);
    let s = sin(i.b.x);
    let local = vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c);
    let t = vec2<f32>(local.x / radius, local.y / (radius * roundness));
    let half = vec2<f32>(radius, max(radius * roundness, 1e-3));
    let reach = (half + QUAD_REACH) / half;
    if (abs(t.x) > reach.x || abs(t.y) > reach.y) {
        discard;
    }
    let uv = t * i.b.yz * half / (half + QUAD_REACH) * 0.5 + 0.5;
    let layer = i32(i.b.w);
    var texel: vec4<f32>;
    if (i.c.w > 0.5) {
        let size = vec2<i32>(textureDimensions(shapes));
        let at = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - 1);
        texel = textureLoad(shapes, at, layer, 0);
    } else {
        // (a brush file's own image is bigger than the brush images, by as many levels)
        let side = f32(textureDimensions(shapes).x);
        let lod = clamp(log2(0.5 * side / (radius + QUAD_REACH)), 0.0, i.d.z + log2(side / 256.0));
        texel = textureSampleLevel(shapes, shape_smp, uv, layer, lod);
    }
    // a picture-like tip covers as far as it is opaque, its grey its own (premultiplied); a mask's
    // grey is the flat middle
    var a = texel.r;
    var grey = FLAT;
    if (i.c.z > 0.5) {
        a = texel.a;
        grey = clamp(texel.r / max(texel.a, 1e-5), 0.0, 1.0);
    }
    // flattened towards the middle as far as the stamp is thin
    grey = FLAT + (grey - FLAT) * clamp(i.c.y, 0.0, 1.0);
    let inside = textureSampleLevel(selected, shape_smp, i.pos.xy / i.d.xy, 0.0).r;
    let alpha = clamp(a * i.c.x * inside, 0.0, 1.0);
    if (alpha <= 0.0) {
        discard;
    }
    return vec4<f32>(vec3<f32>(grey * alpha), alpha);
}

// (bound apart from the stamp's group: numbered on from it)
@group(0) @binding(5) var paint: texture_2d<f32>;
@group(0) @binding(6) var height: texture_2d<f32>;

// The whole target, as one triangle (each pass held to its scissor).
@vertex
fn vs_lit(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// HSL lightness, as an image editor's lightness blending reckons it
fn lightness_of(c: vec3<f32>) -> f32 {
    return 0.5 * (max(c.r, max(c.g, c.b)) + min(c.r, min(c.g, c.b)));
}

// `c` moved to lightness `l`, its hue and saturation kept as far as the colour gamut lets
fn at_lightness(c: vec3<f32>, l: f32) -> vec3<f32> {
    let moved = c + (l - lightness_of(c));
    let m = lightness_of(moved);
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

// The layer's pixel: the paint (premultiplied) at the lightness the height's grey says, a middle
// grey or none keeping its own and each way from it lighter or darker, along the curve through the
// paint's own lightness a lightness tip takes. Its coverage is the paint's.
@fragment
fn fs_lit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let at = vec2<i32>(pos.xy);
    let c = textureLoad(paint, at, 0);
    if (c.a <= 0.0) {
        return vec4<f32>(0.0);
    }
    let h = textureLoad(height, at, 0);
    // (the grey's way from the middle, as far as the height is opaque: premultiplied as it is kept)
    let m = h.r - 0.5 * h.a + 0.5;
    let straight = clamp(c.rgb / c.a, vec3<f32>(0.0), vec3<f32>(1.0));
    let b = 4.0 * lightness_of(straight) - 1.0;
    let l = clamp((1.0 - b) * m * m + b * m, 0.0, 1.0);
    return vec4<f32>(at_lightness(straight, l) * c.a, c.a);
}
