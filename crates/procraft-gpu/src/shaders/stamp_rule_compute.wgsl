// Stamps laid by their stroke's rule in a compute pass (see stamp_rule.wgsl), each pixel written
// once into a buffer copied into the stroke after.

// The part of the canvas worked on: its corner, its tiles across, and the rule (1: gradual,
// 2: towards the transfer); then each tile's (first, count) into the stamp list after them.
@group(0) @binding(1) var<storage, read> rule_stamps: array<f32>;
@group(0) @binding(2) var<storage, read> rule_tiles: array<u32>;
// the stroke as it stands before these stamps, and where the part goes, a row of RULE_ROW
// pixels each, as half floats
@group(0) @binding(3) var rule_before: texture_2d<f32>;
@group(0) @binding(4) var<storage, read_write> rule_out: array<vec2<u32>>;

fn rule_float(b: u32) -> f32 {
    return rule_stamps[b];
}

fn rule_word(i: u32) -> u32 {
    return rule_tiles[i];
}

@compute @workgroup_size(16, 16)
fn cs_stamp_rule(@builtin(workgroup_id) tile: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let corner = vec2<i32>(bitcast<i32>(rule_tiles[0]), bitcast<i32>(rule_tiles[1]));
    let at = tile.xy * RULE_TILE + local.xy;
    let px = corner + vec2<i32>(at);
    let size = vec2<i32>(textureDimensions(rule_before));
    if (px.x >= size.x || px.y >= size.y) {
        return;
    }
    let paint = rule_laid(textureLoad(rule_before, px, 0), px, at);
    let o = at.y * RULE_ROW + at.x;
    rule_out[o] = vec2<u32>(pack2x16float(paint.rg), pack2x16float(paint.ba));
}
