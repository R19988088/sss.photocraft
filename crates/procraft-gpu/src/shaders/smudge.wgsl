// Smudge: the brush carries colour. A grid fixed to the canvas's axes, a pixel a cell, moving
// with the brush a whole pixel at a time, holds what it carries (so nothing is resampled as it
// goes); for each stamp, in order, it picks up some of what is under it (cs_carry) and lays what
// it carries back down under the stamp's shape (cs_deposit). The stamps go down a display frame
// at a time, as wet paint's do: each picks up from the layer as the last pass left it
// (`pass_tex`), so the stamps of one frame see each other only through what the brush carries.
// The part of the layer being smudged that a pass reaches (its window) is read and written through
// the pass's own accessors: `work_at`, `grained_at`, `amount_at`, `shown_at`, `carry_at` and
// `plot_texel`, which smudge_compute.wgsl reads from storage buffers (packed premultiplied RGBA8
// pixels, read and written in place) and smudge_frag.wgsl from textures (on a GPU without compute
// passes, each pass drawn into another texture and copied back). Each pass's work is a function
// here, the pass writing what it gives.

struct SmudgeStamp {
    center: vec2<f32>,
    radius: f32,
    angle: f32,
    roundness: f32,
    // the stamp's alpha times the tool's strength
    alpha: f32,
    // how much of what it carries the brush keeps at each stamp (the rest is picked up)
    pull: f32,
    hardness: f32,
    shape: i32,
    // the cells carried over from the stamp before: those within this many of the middle (-1 at
    // the stroke's first stamp); the rest load what is under them, as a stroke's start does
    kept: i32,
    preserve_alpha: u32,
    // u32 words per row of the window's buffers
    stride: u32,
    // the pixels this stamp can touch: x0, y0, x1, y1
    bbox: vec4<i32>,
    size: vec2<u32>,
    grid: u32,
    // how much of wet paint shows at a pixel, by its glaze: as far as its stamps took it (0), as
    // far as the strongest stamp over it (1), or towards each stamp's transfer, never back (2);
    // the last two kept in `amount`'s first half
    rule: u32,
    // wet mix: the paint (straight colour), how much of it the brush mixes into what it carries,
    // and how much of what it carries it lays down
    paint: vec4<f32>,
    // 0 smudges; 1 paints wet
    mode: u32,
    // what light pressure lets the stamp through to (wet paint building up towards it)
    transfer: f32,
    final_mixture: f32,
    // the mix's grade (-1..1, folded into the mixtures) and the stamp's blur (0..1, see cs_blur_down)
    grade: f32,
    blur: f32,
    // the brush's grain: a layer of `grains` (-1: none), the size of a tile in pixels (negative
    // for a grain moving with the stamp, else fixed to the canvas) and how strongly it shows
    grain: i32,
    grain_scale: f32,
    grain_depth: f32,
    // where a moving grain is read from at the stamp's middle (see the stamp shader), and the
    // grain turned (radians)
    grain_offset: vec2<f32>,
    grain_rotation: f32,
    // 1: the grain read unfiltered (sharp)
    grain_sharp: f32,
    // 1: the shape read unfiltered (sharp)
    shape_sharp: f32,
    // how hard the paint is pressed into a moving grain, and how much that counts
    grain_press: f32,
    grain_bleed: f32,
    // 1: paint laid in place with a texturized grain, which cs_synthesize lays over the stroke once
    grain_once: f32,
    // where the grid's middle cell is on the canvas (a whole pixel when a cell is one), canvas
    // pixels a cell (1 unless the stroke's first stamp was wider than the grid), and this stamp's
    // cells: those within `half` of the middle
    anchor: vec2<f32>,
    scale: f32,
    half: i32,
    // wet paint (see cs_wet_plots): the stamps in the pass, their plots' texels a side, this
    // stamp's place in the pass (for the plots pass: which carried-over plot it reads), and the
    // texels a side of the plot the pass starts from
    count: u32,
    plot: u32,
    index: u32,
    carried: u32,
    // 1: wet paint builds up by what is left at each pixel (the modulated mode; see `amount`)
    modulated: u32,
    // how the moving grain combines with the shape (a blend mode), for paint laid in place
    grain_blend: u32,
    // a dual brush's paint laid in place: the dual mode + 1 (0: none, the paint laid alone) and
    // the second's threshold (negative: none), for cs_synthesize
    dual: u32,
    dual_threshold: f32,
    // 1: the stamp only adds to the paint laid so far (`amount`'s second half), which
    // cs_synthesize lays over the layer as it was; 2: wet paint, which also keeps what it lays in
    // `grained`, so that the brush goes on mixing with the stroke as it was laid
    laid_apart: u32,
    // the coarsest mip level the shape is read from
    shape_lod_cap: f32,
    // 1: wet paint mixes with the layer as it was before the stroke (a glaze), not as it goes
    source_before: u32,
    // the grain's levels, as the stamp shader takes them
    grain_brightness: f32,
    grain_contrast: f32,
    // how far paint laid in place pools, as cs_synthesize lays it (see the composite shader)
    wet_edges: f32,
    // wet paint with luminance blending: 1 when its colours mix squared, 2 when they do as it goes
    // over the layer too (paint that mixes only with the layer as it was)
    luminance: u32,
    // pixels a side of the image the grain is read from
    grain_side: f32,
    // how far paint laid apart burns the paint it goes over, and by what blend mode
    burnt: f32,
    burnt_blend: u32,
    // -1 where the stamp's shape is mirrored on that axis, else 1
    flip_x: f32,
    flip_y: f32,
    // the window's rows (the amount's second half starts `stride * rows` words in), and the canvas
    // pixel at its first word
    rows: u32,
    origin_x: i32,
    origin_y: i32,
    // the part of a dispatch's cells or plot texels this one does: from (x0, y0) up to (x1, y1), a
    // byte each from the lowest
    part: u32,
    // what its shapes are (the canvas's TipKind): all but masks (0) RGBA, covering as far as they
    // are opaque
    tip_kind: u32,
    // a stamp picking up behind it: the share of the paint mixed into what it carries, where the
    // grid's middle cell picks up from (else the anchor), and (above 0) what it carries made one
    // colour, the average of the cells within this share of its reach of the middle, weighed by
    // the stamp's shape (1) or not (0)
    paint_rate: f32,
    pick: vec2<f32>,
    dull: f32,
    dull_weighted: u32,
    // 1: what it carries laid over what is there (never thinning it), else in its place
    smear_over: u32,
    // 1: a smudge picks up from the layer as the last pass left it (`pass_tex`, kept from canvas
    // pixel `pass_origin` on); 0: from the layer as it is
    passed: u32,
    pass_origin: vec2<i32>,
    // 1: it picks up from the image as last shown (every layer), else from its layer
    source_image: u32,
    // what it picks up comes through a warp: its kind (0 none, 1 scale, 2 turn, 3 shift, 4 lens
    // out, 5 lens in, 6 scatter; 16 added: the nearest pixel), its shift and amount (see
    // `warped`); and how much it is sharpened against its blur
    warp_kind: u32,
    warp_shift: vec2<f32>,
    warp_amount: f32,
    sharpen: f32,
    // 颜料混合: how strongly the paint mixes with the layer as a pigment (0..1; 0 without the table)
    pigment: f32,
    pigment_pad0: f32,
    pigment_pad1: f32,
    pigment_pad2: f32,
}

// a pass of wet paint's stamps: middle, radius and turn; roundness, pull, mixture and final
// mixture
struct WetStamp {
    at: vec4<f32>,
    mixing: vec4<f32>,
    // what the stamp paints with, its colour dynamics in it
    paint: vec4<f32>,
}

struct WetPass {
    stamps: array<WetStamp, 64>,
}

@group(0) @binding(0) var<uniform> st: SmudgeStamp;
@group(0) @binding(5) var<uniform> wet_pass: WetPass;
// a dual brush's paint laid in place: its second's coverage, and the layer as it was
@group(0) @binding(6) var second_tex: texture_2d<f32>;
@group(0) @binding(7) var before_tex: texture_2d<f32>;
// a smudge: the layer as its last pass left it, which its stamps pick up from
@group(0) @binding(8) var pass_tex: texture_2d<f32>;
// the image as last shown, the canvas's size (a stroke picking up from every layer)
@group(0) @binding(9) var image_tex: texture_2d<f32>;
@group(1) @binding(0) var shapes: texture_2d_array<f32>;
@group(1) @binding(1) var grains: texture_2d_array<f32>;
@group(1) @binding(2) var shape_smp: sampler;
@group(1) @binding(3) var grain_smp: sampler;
// how much of each canvas pixel is selected (all of it without a selection, a single texel): paint
// is moved only as far as that, as a brush lays it
@group(1) @binding(4) var selected: texture_2d<f32>;

fn selected_at(p: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(selected)) - 1;
    return textureLoad(selected, clamp(p, vec2<i32>(0), last), 0).r;
}

// whether the selection leaves canvas pixel `p` out: then nothing is laid there, so not even the
// rounding of what is laid can move it
fn left_out(p: vec2<i32>) -> bool {
    return selected_at(p) <= 0.0;
}

// the layer as it was before the stroke at canvas pixel `p`
fn before_at(p: vec2<i32>) -> vec4<f32> {
    return textureLoad(before_tex, p - vec2<i32>(st.origin_x, st.origin_y), 0);
}

// cell or texel `id` of the dispatch's part, and whether it is one of the part's (and of the `n`
// a side)
fn part_at(id: vec2<u32>) -> vec2<u32> {
    return id + vec2<u32>(st.part & 255u, (st.part >> 8u) & 255u);
}

fn in_part(i: vec2<u32>, n: u32) -> bool {
    let end = min(vec2<u32>((st.part >> 16u) & 255u, st.part >> 24u), vec2<u32>(n));
    return i.x < end.x && i.y < end.y;
}

// 颜料混合: `plain`'s colour replaced by `paint` mixed into `under` as a pigment, as far as
// `coverage` says it was laid and the setting says paint mixes that way; alpha stays `plain`'s
fn pigment_lay(under: vec4<f32>, paint: vec3<f32>, coverage: f32, plain: vec4<f32>) -> vec4<f32> {
    if (st.pigment <= 0.0001 || under.a <= 0.0001 || coverage <= 0.0 || plain.a <= 0.0001) {
        return plain;
    }
    // paint that is thicker than a trace mixes as a pigment; over bare paper nothing does
    let support = smooth01(under.a, 0.02, 0.22);
    let weight = clamp(st.pigment, 0.0, 1.0) * support;
    if (weight <= 0.0001) {
        return plain;
    }
    let normal = unpremultiply(plain);
    let beneath = unpremultiply(under);
    let amount = clamp(coverage / max(plain.a, 0.0001), 0.0, 1.0);
    let base = normal + (beneath - normal) * support;
    let mixed = normal + (pigment_lerp(base, paint, pigment_share(amount)) - normal) * weight;
    return vec4<f32>(clamp(mixed, vec3<f32>(0.0), vec3<f32>(1.0)) * plain.a, plain.a);
}

// 颜料混合: `paint` mixed into `carried` as a pigment, as far as `amount` says, premultiplied in
// and out — what the brush carries taking in the paint it lays, or the layer it picks up
fn pigment_carry(carried: vec4<f32>, paint: vec4<f32>, amount: f32) -> vec4<f32> {
    let plain = mix(carried, paint, amount);
    if (st.pigment <= 0.0001 || carried.a <= 0.0001 || paint.a <= 0.0001 || plain.a <= 0.0001) {
        return plain;
    }
    let weight = clamp(st.pigment, 0.0, 1.0);
    let normal = unpremultiply(plain);
    let left = unpremultiply(carried);
    let right = unpremultiply(paint);
    // the same share a plain brush's paint takes over what it goes over: the brush's strength says
    // how much colour it moves, not how much less it mixes
    let mixed = normal + (pigment_lerp(left, right, pigment_share(amount)) - normal) * weight;
    return vec4<f32>(clamp(mixed, vec3<f32>(0.0), vec3<f32>(1.0)) * plain.a, plain.a);
}

fn pixel(x: i32, y: i32) -> vec4<f32> {
    let c = vec2<i32>(clamp(x, 0, i32(st.size.x) - 1), clamp(y, 0, i32(st.size.y) - 1));
    // a stroke picking up from every layer reads the image as last shown
    if (st.source_image == 1u) {
        return textureLoad(image_tex, c, 0);
    }
    // a glaze picks up the layer as it was before the stroke
    if (st.source_before == 1u) {
        return before_at(c);
    }
    // the brush picks up the stroke as it is without the grain laid over it, or its second
    if (st.laid_apart == 2u) {
        return grained_at(c);
    }
    return work_at(c);
}

// the layer at a canvas position (pixel centres at +0.5), bilinear
fn layer_at(q: vec2<f32>) -> vec4<f32> {
    let p = q - 0.5;
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let top = mix(pixel(i.x, i.y), pixel(i.x + 1, i.y), f.x);
    let bottom = mix(pixel(i.x, i.y + 1), pixel(i.x + 1, i.y + 1), f.x);
    return mix(top, bottom, f.y);
}

// the layer as the last pass left it at canvas pixel (x, y) (clamped to the canvas)
fn passed_pixel(x: i32, y: i32) -> vec4<f32> {
    let c = vec2<i32>(clamp(x, 0, i32(st.size.x) - 1), clamp(y, 0, i32(st.size.y) - 1));
    let kept = vec2<i32>(textureDimensions(pass_tex)) - 1;
    return textureLoad(pass_tex, clamp(c - st.pass_origin, vec2<i32>(0), kept), 0);
}

// what a smudge picks up at a canvas position (pixel centres at +0.5), bilinear: the layer as the
// last pass left it, or as it is
fn picked_at(q: vec2<f32>) -> vec4<f32> {
    // (from every layer: the image as last shown, whatever the passes)
    if (st.passed == 0u || st.source_image == 1u) {
        return layer_at(q);
    }
    let p = q - 0.5;
    let i = vec2<i32>(floor(p));
    let f = p - floor(p);
    let top = mix(passed_pixel(i.x, i.y), passed_pixel(i.x + 1, i.y), f.x);
    let bottom = mix(passed_pixel(i.x, i.y + 1), passed_pixel(i.x + 1, i.y + 1), f.x);
    return mix(top, bottom, f.y);
}

// the grid's middle cell
fn middle() -> i32 {
    return i32(st.grid) / 2;
}

// this stamp's cell `id` across its window, as a cell of the grid
fn window_cell(id: vec2<u32>) -> vec2<i32> {
    return vec2<i32>(id) + vec2<i32>(middle() - st.half);
}

// the cells a stamp's window has a side
fn window_side() -> u32 {
    return u32(2 * st.half + 1);
}

// where cell `c` of the grid is in the carry (its first grid)
fn cell_index(c: vec2<i32>) -> i32 {
    return c.y * i32(st.grid) + c.x;
}

// what the brush carries at its window's cell `i` (one of the part's) once this stamp has picked up
fn carry_cell(i: vec2<u32>) -> vec4<f32> {
    let c = window_cell(i);
    // the cell's middle on the canvas: a pixel's middle when a cell is one (picked up where the
    // stamp says, its own place unless it picks up behind it), through its warp
    var at = st.pick + warped((vec2<f32>(c - vec2<i32>(middle())) + 0.5) * st.scale, c);
    if (st.warp_kind >= 16u) {
        at = floor(at) + 0.5;
    }
    let under = picked_at(at);
    let off = abs(c - vec2<i32>(middle()));
    var carried = under;
    if (max(off.x, off.y) <= st.kept) {
        carried = mix(under, carry_at(cell_index(c)), st.pull);
    }
    // the paint mixed in (made one colour first, it is mixed in then), clear paint taking it away
    if (st.paint_rate > 0.0 && st.dull <= 0.0) {
        carried = mix(carried, vec4<f32>(st.paint.rgb * st.paint.a, st.paint.a), st.paint_rate);
    }
    return carried;
}

// A number 0..1 from a cell and the stamp's place, for a warp that scatters.
fn warp_hash(c: vec2<i32>, salt: u32) -> vec2<f32> {
    var h = u32(c.x) * 0x8da6b343u ^ u32(c.y) * 0xd8163841u ^ salt * 0xcb1ab31fu;
    h = (h ^ (h >> 16u)) * 0x7feb352du;
    h = (h ^ (h >> 15u)) * 0x846ca68bu;
    let k = (h ^ (h >> 16u)) * 0x9e3779b9u;
    return vec2<f32>(f32(h >> 8u), f32(k >> 8u)) / 16777216.0;
}

// Where the cell `d` from the stamp's middle (canvas pixels) picks up from, from the middle: as it
// lies, or through the stamp's warp within its outline (`r` 0 at the middle, 1 at the rim, across
// and down its turn and squash).
fn warped(d: vec2<f32>, c: vec2<i32>) -> vec2<f32> {
    let kind = st.warp_kind & 15u;
    if (kind == 0u) {
        return d;
    }
    let co = cos(st.angle);
    let si = sin(st.angle);
    let local = vec2<f32>(d.x * co + d.y * si, -d.x * si + d.y * co);
    let r = length(vec2<f32>(local.x / st.radius, local.y / (st.radius * st.roundness)));
    if (r > 1.0) {
        return d;
    }
    let a = st.warp_amount;
    if (kind == 1u) {
        let s = sign(a) * (abs((1.0 - r) * a) + r);
        if (abs(s) < 1e-4) {
            return d;
        }
        return d / s;
    }
    if (kind == 2u) {
        let t = -a * (1.0 - r);
        return vec2<f32>(d.x * cos(t) - d.y * sin(t), d.x * sin(t) + d.y * cos(t));
    }
    if (kind == 3u) {
        return d - st.warp_shift * (1.0 - r);
    }
    if (kind == 4u || kind == 5u) {
        let n = d / max(st.radius, 1e-3);
        let k = 1.0 + a * dot(n, n);
        if (kind == 4u) {
            return n * k * st.radius;
        }
        return n / max(k, 1e-4) * st.radius;
    }
    let salt = bitcast<u32>(st.center.x) ^ (bitcast<u32>(st.center.y) * 0x9e3779b9u);
    return d + a * (2.0 * warp_hash(c, salt) - 1.0);
}

// What the brush carries made one colour (a dulling stamp): the average of the cells within the
// stamp's share of its reach of the middle (premultiplied, weighed by the stamp's shape there if it
// says), the paint mixed in, kept where the third grid starts for cs_deposit to lay. Summed in 64
// parts, part `i` taking every 64th cell from its own, then the parts summed in order.
const DULL_PARTS: u32 = 64u;

struct DullSum {
    sum: vec4<f32>,
    weight: f32,
}

fn dull_part(i: u32) -> DullSum {
    let n = window_side();
    let reach = f32(st.half) * st.dull;
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var k = i; k < n * n; k = k + DULL_PARTS) {
        let c = window_cell(vec2<u32>(k % n, k / n));
        let off = vec2<f32>(c - vec2<i32>(middle()));
        if (max(abs(off.x), abs(off.y)) > reach) {
            continue;
        }
        var w = 1.0;
        if (st.dull_weighted == 1u) {
            // the stamp's shape over the cell, as it lies where it picks up
            let d = (off + 0.5) * st.scale;
            let co = cos(st.angle);
            let si = sin(st.angle);
            let local = vec2<f32>(d.x * co + d.y * si, -d.x * si + d.y * co);
            w = mask(vec2<f32>(local.x / st.radius, local.y / (st.radius * st.roundness)));
        }
        sum = sum + carry_at(cell_index(c)) * w;
        weight = weight + w;
    }
    return DullSum(sum, weight);
}

// the one colour, from the parts' sums added up in order
fn dull_colour(total: vec4<f32>, weighed: f32) -> vec4<f32> {
    var one = carry_at(cell_index(vec2<i32>(middle())));
    if (weighed > 1e-6) {
        one = total / weighed;
    }
    return mix(one, vec4<f32>(st.paint.rgb * st.paint.a, st.paint.a), st.paint_rate);
}

// where the one colour is kept: where the third grid starts
fn dull_index() -> i32 {
    return 2 * i32(st.grid) * i32(st.grid);
}

// The stamp's blur: what the brush lays down this stamp blurred (a Gaussian as wide in canvas
// pixels as the blur says: at 1, 32 pixels a side with a sigma of about 21, growing with the
// blur squared), kept to the stamp's own patch. What it carries stays as it was, so the blur
// never builds up from stamp to stamp. The carry buffer holds three grids: the carry, the blur
// down its columns, then across its rows, which the deposit reads.

// how far the blur reaches and its sigma, in cells `cell` canvas pixels wide
fn blur_reach(cell: f32) -> vec2<f32> {
    let a = clamp(st.blur, 0.0, 1.0);
    let reach = clamp(ceil(32.0 * a), 1.0, 32.0);
    let sigma = a / 3.0 * ceil(64.0 * a);
    return vec2<f32>(min(ceil(reach / max(cell, 1e-3)), f32(st.grid)), sigma / max(cell, 1e-3));
}

fn gauss(j: i32, sigma: f32) -> f32 {
    return exp(-0.5 * f32(j * j) / max(sigma * sigma, 1e-6));
}

// the blur down the columns at the window's cell `id`, kept in the second grid
fn blur_down_cell(id: vec2<u32>) -> vec4<f32> {
    let g = i32(st.grid);
    let c = window_cell(id);
    let r = blur_reach(st.scale);
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var j = -i32(r.x); j <= i32(r.x); j++) {
        let y = clamp(c.y + j, middle() - st.half, middle() + st.half);
        let w = gauss(j, r.y);
        sum += carry_at(y * g + c.x) * w;
        weight += w;
    }
    return sum / weight;
}

// then across the rows of that, kept in the third grid
fn blur_across_cell(id: vec2<u32>) -> vec4<f32> {
    let g = i32(st.grid);
    let c = window_cell(id);
    let r = blur_reach(st.scale);
    var sum = vec4<f32>(0.0);
    var weight = 0.0;
    for (var j = -i32(r.x); j <= i32(r.x); j++) {
        let x = clamp(c.x + j, middle() - st.half, middle() + st.half);
        let w = gauss(j, r.y);
        sum += carry_at(g * g + c.y * g + x) * w;
        weight += w;
    }
    var out = sum / weight;
    // sharpened: the difference from the blur added back
    if (st.sharpen > 0.0) {
        let was = carry_at(c.y * g + c.x);
        out = clamp(was + st.sharpen * (was - out), vec4<f32>(0.0), vec4<f32>(1.0));
        out = vec4<f32>(min(out.rgb, vec3<f32>(out.a)), out.a);
    }
    return out;
}

// what the brush lays down at canvas pixel `p`: what it carries there (blurred, when it blurs),
// the cell itself when a cell is a pixel
fn carried(p: vec2<i32>) -> vec4<f32> {
    let g = i32(st.grid);
    // made one colour
    if (st.dull > 0.0) {
        return carry_at(dull_index());
    }
    let base = select(0, 2 * g * g, st.blur > 0.0);
    let at = (vec2<f32>(p) + 0.5 - st.anchor) / st.scale + f32(middle()) - 0.5;
    let i = vec2<i32>(floor(at));
    let f = at - floor(at);
    let least = middle() - st.half;
    let most = middle() + st.half;
    let x0 = clamp(i.x, least, most);
    let x1 = clamp(i.x + 1, least, most);
    let y0 = clamp(i.y, least, most);
    let y1 = clamp(i.y + 1, least, most);
    let top = mix(carry_at(base + y0 * g + x0), carry_at(base + y0 * g + x1), f.x);
    let bottom = mix(carry_at(base + y1 * g + x0), carry_at(base + y1 * g + x1), f.x);
    return mix(top, bottom, f.y);
}

// how far a stamp's quad reaches past the stamp each way (pixels)
const QUAD_REACH: f32 = 0.70710678;

// half the stamp across each way (pixels)
fn stamp_half() -> vec2<f32> {
    return vec2<f32>(st.radius, max(st.radius * st.roundness, 1e-3));
}

fn mask(t: vec2<f32>) -> f32 {
    if (st.shape < 0) {
        let d = length(t) * st.radius;
        let rim = clamp(st.radius - d + 0.5, 0.0, 1.0);
        let soft = 1.0 - smoothstep(min(st.hardness, 0.999) * st.radius, st.radius + 0.5, d);
        return min(rim, soft);
    }
    // the shape fills the stamp's quad, as a brush stamp's does
    let half = stamp_half();
    let uv = t * vec2<f32>(st.flip_x, st.flip_y) * half / (half + QUAD_REACH) * 0.5 + 0.5;
    if (st.shape_sharp > 0.5) {
        // unfiltered: the texel under it
        let size = vec2<i32>(textureDimensions(shapes));
        let at = clamp(vec2<i32>(floor(uv * vec2<f32>(size))), vec2<i32>(0), size - 1);
        let texel = textureLoad(shapes, at, st.shape, 0);
        return select(texel.r, texel.a, st.tip_kind > 0u);
    }
    // (a brush file's own image is bigger than the brush images, by as many levels)
    let side = f32(textureDimensions(shapes).x);
    let lod = clamp(log2(0.5 * side / (st.radius + QUAD_REACH)), 0.0, st.shape_lod_cap + log2(side / 256.0));
    let texel = textureSampleLevel(shapes, shape_smp, uv, st.shape, lod);
    return select(texel.r, texel.a, st.tip_kind > 0u);
}

// how much of the stamp the grain lets through at canvas point `p`
fn grain_at(p: vec2<f32>) -> f32 {
    let g = grain_plain(p);
    if (st.grain_bleed > 0.0) {
        // pressed in as far as the pressure's bleed says
        return mix(1.0, mix(g, pressed_in(g, st.grain_press), min(st.grain_bleed, 1.0)), st.grain_depth);
    }
    return mix(1.0, g, st.grain_depth);
}

// the grain at canvas point `p` as it is, before its depth and bleed
fn grain_plain(p: vec2<f32>) -> f32 {
    if (st.grain < 0) {
        return 1.0;
    }
    let scale = abs(st.grain_scale);
    var at = p;
    if (st.grain_scale < 0.0) {
        // a moving grain: read as far from where the stamp's middle is in it as the pixel is from
        // the middle on the canvas, so a rolling grain stays put as the stamps move over it
        at = p - st.center + st.grain_offset;
    }
    let gc = cos(st.grain_rotation);
    let gs = sin(st.grain_rotation);
    let uv = vec2<f32>(at.x * gc - at.y * gs, at.x * gs + at.y * gc) / scale;
    let sharp = st.grain_sharp > 0.5;
    // a brush file's own image has the fine detail itself; ours only stands in for it
    let own = st.grain_side > 256.0;
    let level = max(log2(st.grain_side / scale), 0.0);
    var g = textureSampleLevel(grains, grain_smp, uv, st.grain, select(level, 0.0, sharp && !own)).r;
    if (sharp) {
        if (own) {
            // unfiltered: the texel under it at its level
            let l = i32(round(level));
            g = textureLoad(grains, vec2<i32>(fract(uv) * vec2<f32>(textureDimensions(grains, l))), st.grain, l).r;
        } else {
            g = sharp_grain(g, uv);
        }
    }
    return clamp((g - 0.5) * (1.0 + st.grain_contrast) + 0.5 + st.grain_brightness, 0.0, 1.0);
}

// how much a wet stamp lays at canvas point `p` (stamp frame point `t`): its shape and its grain
// combined by the grain's blend mode (unless the grain is laid over the stroke once), then the
// bleed as a contrast on that (none of the faint parts, all of the strong ones, more of both the
// harder the pressure's bleed), times the stamp's alpha
fn wet_weight(p: vec2<f32>, t: vec2<f32>) -> f32 {
    var through = mask(t);
    if (st.grain >= 0 && st.grain_once <= 0.5) {
        through = grain_combine(through, grain_plain(p), st.grain_depth, st.grain_blend);
    }
    let b = 8.0 * st.grain_bleed;
    let bled = (1.0 + b) * through - b / sqrt(max(b * st.grain_press, 1e-4));
    return clamp(bled, 0.0, 1.0) * st.alpha;
}

// What a stamp lays at a canvas pixel: the pixel as it leaves it (dithered, before it is rounded to 8
// bits), whether that goes where the stroke keeps what it laid (`grained`) rather than into the
// layer, and how much of the paint the pixel has taken and shows (and whether those changed).
// Nothing laid (outside the stamp, or where it lays nothing): `laid` false.
struct Deposit {
    laid: bool,
    out: vec4<f32>,
    into_grained: bool,
    amount: f32,
    amount_set: bool,
    shown: f32,
    shown_set: bool,
}

// whether canvas pixel `p` is one of the stamp's (within its bounds and the canvas)
fn in_bbox(p: vec2<i32>) -> bool {
    return !(p.x >= st.bbox.z || p.y >= st.bbox.w || p.x < 0 || p.y < 0 || p.x >= i32(st.size.x) || p.y >= i32(st.size.y));
}

fn deposit(p: vec2<i32>) -> Deposit {
    var dep: Deposit;
    dep.laid = false;
    dep.into_grained = st.laid_apart == 2u;
    dep.amount_set = false;
    dep.shown_set = false;
    if (!in_bbox(p) || left_out(p)) {
        return dep;
    }
    // into the stamp's frame
    let d = vec2<f32>(p) + 0.5 - st.center;
    let c = cos(st.angle);
    let s = sin(st.angle);
    let local = vec2<f32>(d.x * c + d.y * s, -d.x * s + d.y * c);
    let t = vec2<f32>(local.x / st.radius, local.y / (st.radius * st.roundness));
    // the stamp's quad
    let half = stamp_half();
    let reach = (half + QUAD_REACH) / half;
    if (abs(t.x) > reach.x || abs(t.y) > reach.y) {
        return dep;
    }
    let shape = mask(t);
    if (shape * st.alpha <= 0.0) {
        return dep;
    }
    // the grain lets each stamp through as far as it stands: stamps over a pixel build up past
    // it towards full, the grain showing while they are few or light
    let g = grain_at(vec2<f32>(p) + 0.5);
    let m = shape * st.alpha;
    let w = select(work_at(p), grained_at(p), st.laid_apart == 2u);
    // smudging: the grain lets less of what is carried through
    let moved = carried(p);
    var out = mix(w, moved, m * g);
    if (st.smear_over == 1u) {
        out = moved * (m * g) + w * (1.0 - moved.a * m * g);
    }
    if (st.mode == 0u) {
        // 颜料混合: what the brush carries mixes with the layer as a pigment (wet paint mixes at
        // its own laying, below)
        out = pigment_lay(w, unpremultiply(moved), m * g, out);
    }
    if (st.mode == 1u) {
        // wet: the stamp's plot mixed in, alpha and all (a stamp can lay bare paper), as far as
        // its weight there
        // the stamp's plot, or (dry paint laid in place, no plots) the paint itself
        var laid = vec4<f32>(st.paint.rgb, 1.0);
        if (st.count > 0u) {
            laid = plot_at(PASS_PLOTS, st.index * st.plot * st.plot, st.plot, t);
        }
        var weight = wet_weight(vec2<f32>(p) + 0.5, t);
        if (st.rule == 1u) {
            // the paint shows as far as the strongest stamp over the pixel
            dep.amount = max(amount_at(p), weight);
            dep.amount_set = true;
        } else if (st.rule == 2u) {
            // towards the stamp's transfer by its cover, never past it and never back; the paint
            // mixed by the cover the transfer leaves
            let held = amount_at(p);
            dep.amount = max(held, held + weight * (st.transfer - held));
            dep.amount_set = true;
            weight = weight * st.transfer;
        }
        if (st.modulated == 1u) {
            // only as far as the pixel has room left, which fills as it takes paint
            let held = select(amount_at(p), dep.amount, dep.amount_set);
            weight = weight * (1.0 - held);
            dep.amount = held + weight * (1.0 - held);
            dep.amount_set = true;
        }
        if (st.laid_apart != 0u) {
            // how much of the paint shows there so far, put together with the rest later
            let held = shown_at(p);
            dep.shown = held + weight * (1.0 - held);
            dep.shown_set = true;
        }
        out = from_mix(mix(to_mix(w), to_mix(laid), weight));
        if (st.laid_apart != 2u) {
            // 颜料混合: the paint mixes with the layer as a pigment where it is laid for good
            // (paint laid apart mixes once, where cs_synthesize shows it)
            out = pigment_lay(w, unpremultiply(laid), weight, out);
        }
    }
    if (st.preserve_alpha == 1u) {
        out = out * (w.a / max(out.a, 1e-6));
    }
    // outside the selection the layer stays as it is
    out = mix(w, out, selected_at(p));
    // rounded to 8 bits up or down at random, in proportion to the remainder, so that faint stamps
    // add up as they should instead of rounding away
    // (wet paint laid apart is kept as it was laid; shown by cs_synthesize)
    let h = dither(u32(p.x), u32(p.y), bitcast<u32>(st.center.x) ^ (bitcast<u32>(st.center.y) * 2654435761u));
    dep.out = clamp(out + (h - 0.5) / 255.0, vec4<f32>(0.0), vec4<f32>(1.0));
    dep.laid = true;
    return dep;
}

// Paint laid in place with a texturized grain or a second, put together where its stamps or its
// second's went: the paint over the layer as it was, as far as how much of it was laid combines
// with the grain by its mode, then the second's coverage (cut at its threshold) by the dual mode. Wet
// paint keeps what it laid: its change to the layer, scaled from how much was laid to that. Only
// this far: more, and the change's own rounding shows as speckle.
const LAID_MOST_SCALE: f32 = 4.0;

// The layer at canvas pixel `p` (one of the stamp's bounds, and selected) once paint laid apart is
// put together there, dithered, before it is rounded to 8 bits.
fn synthesized(p: vec2<i32>) -> vec4<f32> {
    let laid = shown_at(p);
    let was = before_at(p);
    // how much shows: what the stamps laid, or as far as the stroke's glaze built up by its own
    // rule (the paint kept as laid, shown only that far)
    let built = select(laid, min(amount_at(p), laid), st.rule != 0u);
    var a = built;
    if (st.wet_edges > 0.0 && built > 0.0) {
        // the paint pools (see pooled_over), by how much of it shows: wet paint thinned by its
        // water shows less than it touched (what it took of the room the layer had; over a full
        // layer, as far as it touched). Where it took none (over paint as strong as itself) it
        // pools to nothing, as the thinnest paint does
        var shown = built;
        if (st.laid_apart == 2u && was.a < 0.999) {
            let now = grained_at(p).a;
            shown = clamp((now - was.a) / (1.0 - was.a), 0.0, 1.0) * built / max(laid, 1e-6);
        }
        a = built * mix(1.0, pooled_over(shown), st.wet_edges);
    }
    if (st.grain_once > 0.5) {
        a = grain_combine(a, grain_plain(vec2<f32>(p) + 0.5), st.grain_depth, st.grain_blend);
    }
    // a Normal second's share of the colour: where the first laid little, the second shows in the
    // stroke's own colour (none for any other dual mode)
    var toward = 0.0;
    if (st.dual != 0u) {
        var b = textureLoad(second_tex, p, 0).a;
        if (st.dual_threshold >= 0.0) {
            b = select(0.0, 1.0, b > st.dual_threshold);
        }
        a = dual_rise(dual_combine(a, b, dual_mode(st.dual)), built, dual_mode(st.dual));
        if (dual_mode(st.dual) == 0u) {
            toward = clamp((1.0 - laid) * b, 0.0, 1.0);
        }
    }
    var out = mix(was, vec4<f32>(st.paint.rgb, 1.0), a);
    if ((st.dual & DUAL_CLASSIC) != 0u && st.laid_apart == 2u && laid <= 0.0 && toward > 0.0) {
        // the classic combine where the wet first laid nothing: the stroke's colour, by the
        // second's share, added to the layer as it was (all of it kept, as its paint starts out),
        // shown as far as the second does: over paint it tints it, it doesn't cover it
        let added = mix(was, was + vec4<f32>(st.paint.rgb, 1.0) * toward, a);
        out = vec4<f32>(min(added.rgb, vec3<f32>(added.a)), added.a);
    }
    if (st.laid_apart == 2u && laid > 0.0) {
        // what a second adds past that is laid as plain paint, as it is where none was laid at all
        let scale = min(a / laid, LAID_MOST_SCALE);
        var kept = over_layer(was, grained_at(p), scale);
        // (the paint as it was laid, for a Normal second)
        var first = grained_at(p);
        if (st.burnt > 0.0) {
            // burnt edges show where the stroke is thin: where its grain or its second took paint
            // away, or without either wherever it is less than full. The paint itself, blended
            // once more with what was under it (nothing shows where the layer was empty)
            let full = select(1.0, built, st.grain_once > 0.5 || st.dual != 0u);
            let own = clamp(over_layer(was, grained_at(p), 1.0 / laid), vec4<f32>(0.0), vec4<f32>(1.0));
            let burnt = vec4<f32>(unpremultiply(blend(was, own, st.burnt_blend)) * own.a, own.a);
            let burning = mix(own, burnt, clamp(st.burnt * abs(full - a), 0.0, 1.0));
            kept = over_layer(was, burning, laid * scale);
            first = over_layer(was, burning, laid);
        }
        out = mix(kept, vec4<f32>(st.paint.rgb, 1.0), max(a - laid * scale, 0.0));
        if (toward > 0.0) {
            // a Normal second: the paint as laid turned toward the stroke's own colour as far as
            // the second shows where the first laid little, then shown as far as the two together
            // (where the first laid nothing, the second's colour over the layer)
            var turned = mix(first, vec4<f32>(st.paint.rgb, 1.0), toward);
            if ((st.dual & DUAL_CLASSIC) != 0u && turned.a > 0.0) {
                // the classic combine: the colours of the paint as laid, by how much was, and of
                // the stroke, by the second's share, added up (as far as it shows, the same)
                let own = first.rgb / max(first.a, 1e-6);
                let colour = (own * laid + st.paint.rgb * toward) / (laid + toward);
                turned = vec4<f32>(colour * turned.a, turned.a);
            }
            let shown = toward + (1.0 - toward) * laid;
            out = over_layer(was, turned, min(a / max(shown, 1e-6), LAID_MOST_SCALE));
        }
    }
    // 颜料混合: the stroke's paint mixes with the layer as it was as a pigment
    out = pigment_lay(was, st.paint.rgb, a, out);
    if (st.preserve_alpha == 1u) {
        out = out * (was.a / max(out.a, 1e-6));
    }
    out = mix(was, out, selected_at(p));
    // dithered the same each time it is put together again
    let h = dither(u32(p.x), u32(p.y), 0u);
    return clamp(out + (h - 0.5) / 255.0, vec4<f32>(0.0), vec4<f32>(1.0));
}

// a number in 0..1 that looks random, from three
fn dither(a: u32, b: u32, c: u32) -> f32 {
    var h = a * 747796405u + b * 2891336453u + c * 277803737u;
    h = (h ^ (h >> 16u)) * 2246822519u;
    h = (h ^ (h >> 13u)) * 3266489917u;
    h = h ^ (h >> 16u);
    return f32(h >> 8u) / 16777216.0;
}

// Wet paint, a display frame's stamps at a time (`st.count` of them, listed in `wet_pass`). Each
// stamp has a plot, a square of `st.plot` texels over its own turned quad (and a texel more round
// it), holding what it lays down. A thread a texel walks the pass's stamps in order with what the
// brush carries there: it picks up what is under that stamp (the layer as the pass found it) as
// far as the pull leaves room, takes in fresh paint by the mixture, and the stamp's plot is that
// laid over what was under the pass's first stamp by the final mixture. The last stamp's plot is
// what the next pass starts from; the stroke's first starts from what is under it. The carry
// buffer holds two carried-over plots (read one, write the other) and then the pass's plots.

// room for a carried-over plot: the largest plot's texels (128 a side)
const CARRIED: u32 = 16384u;

// which plots `plot_texel` reads: the one the pass carries over from (as `st.index` says), or the
// pass's own
const CARRIED_PLOT: u32 = 0u;
const PASS_PLOTS: u32 = 1u;

// premultiplied paint as it mixes: with luminance blending its colour squared, its alpha as it is
fn to_mix(c: vec4<f32>) -> vec4<f32> {
    if (st.luminance == 0u || c.a <= 0.0) {
        return c;
    }
    let plain = c.rgb / c.a;
    return vec4<f32>(plain * plain * c.a, c.a);
}

// paint that has mixed, back as the layer keeps it
fn from_mix(c: vec4<f32>) -> vec4<f32> {
    if (st.luminance == 0u || c.a <= 0.0) {
        return c;
    }
    return vec4<f32>(sqrt(max(c.rgb, vec3<f32>(0.0)) / c.a) * c.a, c.a);
}

// what paint laid apart shows over the layer `was`: `t` of the way to `laid`
fn over_layer(was: vec4<f32>, laid: vec4<f32>, t: f32) -> vec4<f32> {
    if (st.luminance == 2u) {
        return from_mix(mix(to_mix(was), to_mix(laid), t));
    }
    return was + (laid - was) * t;
}

// the canvas point under stamp `w` at its point `t` (-1..1 across each way)
fn plot_point(w: WetStamp, t: vec2<f32>) -> vec2<f32> {
    let l = vec2<f32>(t.x * w.at.z, t.y * w.at.z * w.mixing.x);
    let c = cos(w.at.w);
    let s = sin(w.at.w);
    return w.at.xy + vec2<f32>(l.x * c - l.y * s, l.x * s + l.y * c);
}

// the stamp point a plot's texel `i` stands for, in a plot `n` texels a side
fn plot_t(i: vec2<f32>, n: u32) -> vec2<f32> {
    return ((i + 0.5) / f32(n) * 2.0 - 1.0) * (1.0 + 1.0 / f32(n));
}

// what plot `base` texels into `plots` (`n` texels a side) holds at stamp point `t`, bilinear
fn plot_at(plots: u32, base: u32, n: u32, t: vec2<f32>) -> vec4<f32> {
    let at = (t / (1.0 + 1.0 / f32(n)) * 0.5 + 0.5) * f32(n) - 0.5;
    let i = vec2<i32>(floor(at));
    let f = at - floor(at);
    let last = i32(n) - 1;
    let x0 = u32(clamp(i.x, 0, last));
    let x1 = u32(clamp(i.x + 1, 0, last));
    let y0 = u32(clamp(i.y, 0, last));
    let y1 = u32(clamp(i.y + 1, 0, last));
    let top = mix(plot_texel(plots, base + y0 * n + x0), plot_texel(plots, base + y0 * n + x1), f.x);
    let bottom = mix(plot_texel(plots, base + y1 * n + x0), plot_texel(plots, base + y1 * n + x1), f.x);
    return mix(top, bottom, f.y);
}

// A pass's plots at plot texel `i`, as a thread walks them: what lies under the pass's first stamp,
// what the brush carries, and the plot of the last stamp walked (in the mix's space).
struct WetWalk {
    under_first: vec4<f32>,
    carried: vec4<f32>,
    out: vec4<f32>,
}

fn wet_walk_start(t: vec2<f32>) -> WetWalk {
    // (what the brush carries stays as it mixes from pass to pass; only the plots go back)
    var walk: WetWalk;
    walk.under_first = to_mix(layer_at(plot_point(wet_pass.stamps[0], t)));
    walk.carried = walk.under_first;
    if (st.kept >= 0) {
        walk.carried = plot_at(CARRIED_PLOT, 0u, st.carried, t);
    }
    walk.out = walk.carried;
    return walk;
}

// the walk on past the pass's stamp `k`
fn wet_walk_on(walk: WetWalk, k: u32, t: vec2<f32>) -> WetWalk {
    let w = wet_pass.stamps[k];
    var on = walk;
    // 颜料混合: what the brush picks up and the paint it takes in mix as pigments, so that a
    // brush carrying yellow and laying blue carries green
    on.carried = pigment_carry(on.carried, to_mix(layer_at(plot_point(w, t))), 1.0 - w.mixing.y);
    on.carried = pigment_carry(on.carried, to_mix(w.paint), w.mixing.z);
    // 颜料混合: the plot put down over the layer as a pigment too, so that piling it on mixes
    // once as a pigment rather than mixing plain and then mixing it again
    on.out = pigment_carry(walk.under_first, on.carried, w.mixing.w);
    return on;
}
