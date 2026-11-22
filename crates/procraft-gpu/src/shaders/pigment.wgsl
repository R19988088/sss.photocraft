// 颜料混合: paint and what it goes over mixed as pigments rather than as light, by the table Prefs'
// pigment mixing strength reads — 38 bands of Kubelka-Munk response over the colour cube, kept as a
// 3D texture (32 × 32 points to a band's 32 slices, ten bands on the depth axis) so that the GPU
// filters it by its own hardware and no hand interpolation is needed.

// points a side of the cube, and bands the response is held in
const PIGMENT_GRID: f32 = 32.0;
const PIGMENT_BANDS: u32 = 10u;
const PIGMENT_SLICES: f32 = 32.0;
// each of the table's two halves holds five bands' 32 slices (an old GPU's 3D textures stop at 256
// layers, where one of all ten would take 320)
const PIGMENT_HALF_BANDS: u32 = 5u;
const PIGMENT_HALF_DEPTH: f32 = 160.0;

// the table in two halves, and the sampler the hardware interpolates it with (12 and 14 for the
// halves, 13 the sampler: the bindings free in both groups it is read in, the composite's quad
// group and a smudge's compute group)
@group(0) @binding(12) var pigment_lut: texture_3d<f32>;
@group(0) @binding(14) var pigment_lut_b: texture_3d<f32>;
@group(0) @binding(13) var pigment_sampler: sampler;

// the CIE 1931 matching functions, a band a row
const CMF: array<vec3<f32>, 38> = array<vec3<f32>, 38>(
    vec3<f32>(0.000064692, 0.0000018443, 0.0003050171),
    vec3<f32>(0.0002194099, 0.0000062053, 0.0010368067),
    vec3<f32>(0.0011205744, 0.0000310096, 0.005313136),
    vec3<f32>(0.0037666134, 0.0001047484, 0.017954393),
    vec3<f32>(0.011880553, 0.0003536405, 0.057077582),
    vec3<f32>(0.023286443, 0.0009514714, 0.11365162),
    vec3<f32>(0.034559418, 0.0022822632, 0.17335873),
    vec3<f32>(0.03722379, 0.004207329, 0.19620657),
    vec3<f32>(0.032418376, 0.0066887984, 0.18608238),
    vec3<f32>(0.021233205, 0.009888396, 0.13995047),
    vec3<f32>(0.010490991, 0.015249452, 0.08917453),
    vec3<f32>(0.0032958376, 0.02141831, 0.04789621),
    vec3<f32>(0.0005070352, 0.033422932, 0.028145626),
    vec3<f32>(0.0009486742, 0.051310014, 0.016137662),
    vec3<f32>(0.006273718, 0.070402084, 0.0077591019),
    vec3<f32>(0.016864624, 0.08783871, 0.004296148),
    vec3<f32>(0.028689649, 0.094249054, 0.002005509),
    vec3<f32>(0.042674812, 0.09795667, 0.0008614711),
    vec3<f32>(0.05625475, 0.09415218, 0.0003690387),
    vec3<f32>(0.0694704, 0.086781024, 0.0001914287),
    vec3<f32>(0.08305315, 0.078856534, 0.0001495556),
    vec3<f32>(0.0861261, 0.063526703, 0.0000923109),
    vec3<f32>(0.090466134, 0.053741417, 0.0000681349),
    vec3<f32>(0.08500387, 0.042646064, 0.0000288264),
    vec3<f32>(0.07090667, 0.03161735, 0.0000157672),
    vec3<f32>(0.050628892, 0.020885206, 0.0000039406),
    vec3<f32>(0.03547396, 0.01386011, 0.000001584),
    vec3<f32>(0.021468211, 0.0081026402, 0.0),
    vec3<f32>(0.012516457, 0.0046301023, 0.0),
    vec3<f32>(0.0068045816, 0.00249138, 0.0),
    vec3<f32>(0.0034645658, 0.0012593034, 0.0),
    vec3<f32>(0.0014976098, 0.0005416465, 0.0),
    vec3<f32>(0.0007697005, 0.0002779529, 0.0),
    vec3<f32>(0.0004073681, 0.0001471081, 0.0),
    vec3<f32>(0.0001690104, 0.0000610327, 0.0),
    vec3<f32>(0.0000952245, 0.0000343873, 0.0),
    vec3<f32>(0.000049031, 0.000017706, 0.0),
    vec3<f32>(0.00002, 0.000007221, 0.0),
);

// a rise from 0 at `a` to 1 at `b`
fn smooth01(x: f32, a: f32, b: f32) -> f32 {
    let t = clamp((x - a) / (b - a), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// the matching function of the table's `band`
fn matching(band: u32) -> vec3<f32> {
    return CMF[band];
}

// a colour from the linear light the bands add up to, as sRGB shows it
fn srgb_of(v: f32) -> f32 {
    if (v < 0.0031308) {
        return clamp(max(v, 0.0) * 12.92, 0.0, 1.0);
    }
    return clamp(1.055 * pow(v, 1.0 / 2.4) - 0.055, 0.0, 1.0);
}

// the table's channels at a colour: 38 bands' response (K/S, as log2 of it plus 1), the 39th the
// paint's weight there; the cube's points are read between by the hardware's filtering
fn pigment_channels(rgb: vec3<f32>) -> array<f32, 40> {
    let at = clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let xy = (at.xy * (PIGMENT_GRID - 1.0) + 0.5) / PIGMENT_GRID;
    let z_at = at.z * (PIGMENT_SLICES - 1.0);
    var channels: array<f32, 40>;
    for (var band = 0u; band < PIGMENT_BANDS; band = band + 1u) {
        // half the table it is in, `band` counted within that half
        let in_b = band >= PIGMENT_HALF_BANDS;
        let k = band - select(0u, PIGMENT_HALF_BANDS, in_b);
        let z = (f32(k * 32u) + z_at + 0.5) / PIGMENT_HALF_DEPTH;
        var values: vec4<f32>;
        if (in_b) {
            values = textureSampleLevel(pigment_lut_b, pigment_sampler, vec3<f32>(xy.x, xy.y, z), 0.0);
        } else {
            values = textureSampleLevel(pigment_lut, pigment_sampler, vec3<f32>(xy.x, xy.y, z), 0.0);
        }
        for (var channel = 0u; channel < 4u; channel = channel + 1u) {
            let slot = band * 4u + channel;
            if (slot < 38u) {
                channels[slot] = max(0.0, exp2(values[channel]) - 1.0);
            } else if (slot == 38u) {
                channels[38] = clamp(values[channel], 0.0, 1.0);
            }
        }
    }
    return channels;
}

// `under` and `paint` mixed as pigments, `t` the part of `paint` (0..1): the bands' K/S added up,
// the light they reflect made up, and back as an sRGB colour
fn pigment_mix(under: vec3<f32>, paint: vec3<f32>, t: f32) -> vec3<f32> {
    if (t <= 0.000001) {
        return under;
    }
    if (t >= 0.999999) {
        return paint;
    }
    let a = pigment_channels(under);
    let b = pigment_channels(paint);
    // the two paints weighed by how much pigment each lays, by thickness alone: the reference weighs
    // them by their own lightness as well, which would have a bright yellow swallow a blue laid on
    // it and never give the green the two make. Equal thicknesses weigh the same, so blue over yellow
    // is green, and a second stroke of blue takes it bluer still
    let wa = (1.0 - t) * (1.0 - t);
    let wb = t * t;
    let sum = wa + wb;
    if (sum <= 0.000001) {
        return select(under, paint, t >= 0.5);
    }
    var light = vec3<f32>(0.0);
    for (var band = 0u; band < 38u; band = band + 1u) {
        let ks = (a[band] * wa + b[band] * wb) / sum;
        light = light + (1.0 / (1.0 + ks + sqrt(ks * ks + 2.0 * ks))) * matching(band);
    }
    let xyz = light;
    let rgb = vec3<f32>(
        3.2409699419 * xyz.x - 1.5373831776 * xyz.y - 0.4986107603 * xyz.z,
        -0.9692436363 * xyz.x + 1.8759675015 * xyz.y + 0.0415550574 * xyz.z,
        0.0556300797 * xyz.x - 0.2039769589 * xyz.y + 1.0569715142 * xyz.z,
    );
    return vec3<f32>(srgb_of(rgb.x), srgb_of(rgb.y), srgb_of(rgb.z));
}

// 颜料混合: how much of the mix a stroke's paint takes. A stroke one layer thick lays as much
// pigment as the paint it goes over, so blue laid over yellow carries green instead of covering it,
// and a second stroke of the same blue turns it bluer still (as a pigment would)
fn pigment_share(t: f32) -> f32 {
    let v = clamp(t, 0.0, 1.0);
    return v / (1.0 + v);
}

// `paint` laid over `under` at `t`, as the paint's own colour where it covers thinly and as the
// pigment mix where it is thicker (`gate`), so that faint paint keeps the plain mix
fn pigment_lerp(under: vec3<f32>, paint: vec3<f32>, t: f32) -> vec3<f32> {
    let plain = under + (paint - under) * t;
    if (t <= 0.025) {
        return plain;
    }
    let mixed = pigment_mix(under, paint, t);
    return plain + (mixed - plain) * smooth01(t, 0.025, 0.16);
}

// the stroke's paint mixed into the colour under it as a pigment, as far as `strength` says and
// only where paint is already there: `plain` is the stroke composited as it would be, `b` the layer
// under it, `s` the stroke's paint
fn pigment_into(b: vec4<f32>, s: vec4<f32>, plain: vec4<f32>, strength: f32, on: u32) -> vec4<f32> {
    if (on == 0u || strength <= 0.0001 || s.a <= 0.0) {
        return plain;
    }
    let support = smooth01(b.a, 0.02, 0.22);
    let weight = clamp(strength, 0.0, 1.0) * support;
    if (weight <= 0.0001) {
        return plain;
    }
    let normal = unpremultiply(plain);
    let under = unpremultiply(b);
    let paint = unpremultiply(s);
    // the paint's own share of the result: what it lays down against what is already there
    let t = clamp(s.a / max(plain.a, 0.0001), 0.0, 1.0);
    let base = normal + (under - normal) * support;
    let mixed = normal + (pigment_lerp(base, paint, pigment_share(t)) - normal) * weight;
    return vec4<f32>(clamp(mixed, vec3<f32>(0.0), vec3<f32>(1.0)) * plain.a, plain.a);
}
