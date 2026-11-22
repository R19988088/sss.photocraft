// Smudge's passes as compute passes (see smudge.wgsl): the window is a buffer of packed
// premultiplied RGBA8 pixels, read and written in place, and what the brush carries a buffer of
// its own.

@group(0) @binding(1) var<storage, read_write> work: array<u32>;
@group(0) @binding(2) var<storage, read_write> carry: array<vec4<f32>>;
// wet paint laid apart: the stroke as it was laid, without its grain or its second
@group(0) @binding(3) var<storage, read_write> grained: array<u32>;
// wet paint in the modulated mode: how much each pixel has taken so far; and (its second half) how
// much of the paint laid apart shows
@group(0) @binding(4) var<storage, read_write> amount: array<f32>;

// where canvas pixel `p` is in the window's buffers
fn window_index(p: vec2<i32>) -> u32 {
    return u32(p.y - st.origin_y) * st.stride + u32(p.x - st.origin_x);
}

fn work_at(p: vec2<i32>) -> vec4<f32> {
    return unpack4x8unorm(work[window_index(p)]);
}

fn grained_at(p: vec2<i32>) -> vec4<f32> {
    return unpack4x8unorm(grained[window_index(p)]);
}

fn amount_at(p: vec2<i32>) -> f32 {
    return amount[window_index(p)];
}

fn shown_at(p: vec2<i32>) -> f32 {
    return amount[st.stride * st.rows + window_index(p)];
}

fn carry_at(k: i32) -> vec4<f32> {
    return carry[k];
}

fn plot_texel(plots: u32, k: u32) -> vec4<f32> {
    if (plots == CARRIED_PLOT) {
        return carry[st.index * CARRIED + k];
    }
    return carry[2u * CARRIED + k];
}

@compute @workgroup_size(8, 8)
fn cs_carry(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = part_at(id.xy);
    if (!in_part(i, window_side())) {
        return;
    }
    carry[cell_index(window_cell(i))] = carry_cell(i);
}

var<workgroup> dull_sums: array<vec4<f32>, 64>;
var<workgroup> dull_weights: array<f32, 64>;

@compute @workgroup_size(64)
fn cs_dull(@builtin(local_invocation_index) i: u32) {
    let part = dull_part(i);
    dull_sums[i] = part.sum;
    dull_weights[i] = part.weight;
    workgroupBarrier();
    if (i != 0u) {
        return;
    }
    var total = vec4<f32>(0.0);
    var weighed = 0.0;
    for (var k = 0u; k < DULL_PARTS; k = k + 1u) {
        total = total + dull_sums[k];
        weighed = weighed + dull_weights[k];
    }
    carry[dull_index()] = dull_colour(total, weighed);
}

@compute @workgroup_size(8, 8)
fn cs_blur_down(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = window_side();
    if (id.x >= n || id.y >= n) {
        return;
    }
    let g = i32(st.grid);
    carry[g * g + cell_index(window_cell(id.xy))] = blur_down_cell(id.xy);
}

@compute @workgroup_size(8, 8)
fn cs_blur_across(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = window_side();
    if (id.x >= n || id.y >= n) {
        return;
    }
    let g = i32(st.grid);
    carry[2 * g * g + cell_index(window_cell(id.xy))] = blur_across_cell(id.xy);
}

@compute @workgroup_size(8, 8)
fn cs_deposit(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = st.bbox.xy + vec2<i32>(id.xy);
    let d = deposit(p);
    if (!d.laid) {
        return;
    }
    let k = window_index(p);
    if (d.amount_set) {
        amount[k] = d.amount;
    }
    if (d.shown_set) {
        amount[st.stride * st.rows + k] = d.shown;
    }
    if (d.into_grained) {
        grained[k] = pack4x8unorm(d.out);
        return;
    }
    work[k] = pack4x8unorm(d.out);
}

@compute @workgroup_size(8, 8)
fn cs_synthesize(@builtin(global_invocation_id) id: vec3<u32>) {
    let p = st.bbox.xy + vec2<i32>(id.xy);
    if (!in_bbox(p) || left_out(p)) {
        return;
    }
    work[window_index(p)] = pack4x8unorm(synthesized(p));
}

@compute @workgroup_size(8, 8)
fn cs_wet_plots(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = st.plot;
    let i = part_at(id.xy);
    if (!in_part(i, n)) {
        return;
    }
    let t = plot_t(vec2<f32>(i), n);
    var walk = wet_walk_start(t);
    for (var k = 0u; k < st.count; k++) {
        walk = wet_walk_on(walk, k, t);
        carry[2u * CARRIED + k * n * n + i.y * n + i.x] = from_mix(walk.out);
    }
    carry[(1u - st.index) * CARRIED + i.y * n + i.x] = walk.out;
}
