// Which tiles of a texture a stroke reaches, on a GPU without compute passes: a fragment a tile,
// marked (white) when any texel of it within the region holds anything.

struct Region {
    // the first texel looked at (a multiple of 8 each way), and how many from there
    origin: vec2<u32>,
    size: vec2<u32>,
    // a tile's side (texels), and how many tiles across the texture
    tile: u32,
    across: u32,
    pad: vec2<u32>,
}

@group(0) @binding(0) var<uniform> region: Region;
@group(0) @binding(1) var stroke: texture_2d<f32>;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_marks(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let t = vec2<u32>(pos.xy);
    let lo = max(t * region.tile, region.origin);
    let hi = min((t + 1u) * region.tile, region.origin + region.size);
    for (var y = lo.y; y < hi.y; y++) {
        for (var x = lo.x; x < hi.x; x++) {
            if (any(textureLoad(stroke, vec2<i32>(vec2<u32>(x, y)), 0) != vec4<f32>(0.0))) {
                return vec4<f32>(1.0);
            }
        }
    }
    return vec4<f32>(0.0);
}
