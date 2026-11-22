// What a plugin's picture function is joined to, interface 6: as interface 2's, and given its
// plugin's controls too, as `v`, read as a filter reads its own. Kept as it is once released.
// `/*VALUES*/` becomes the fields of `HostValues`, `/*READ*/` the values from the uniform; the
// picture's text comes after all of this, so nothing it holds can reach back into it. Everything here
// named host_ is the app's, and the text can't touch it (plugin_filter.rs); `v`, `hue` and
// `picture_size` are what it is given, and `picture` is what it brings.

struct HostUniform {
    // the controls' values, in the manifest's order
    a: vec4<f32>,
    b: vec4<f32>,
    c: vec4<f32>,
    d: vec4<f32>,
    // (the hue 0..1, unused, unused, unused)
    h: vec4<f32>,
    // (the picture's width and height in pixels, unused, unused)
    size: vec4<f32>,
}

@group(0) @binding(0) var<uniform> host_u: HostUniform;

struct HostValues { /*VALUES*/ }
var<private> v: HostValues;

// What this pixel may still spend; -1 once it ran out. Every loop pays from it as it goes round.
var<private> host_budget: i32 = /*BUDGET*/;

@vertex
fn host_vs(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// The hue the picture is drawn for, 0 to 1 round the wheel from red.
fn hue() -> f32 {
    return host_u.h.x;
}

fn picture_size() -> vec2<f32> {
    return host_u.size.xy;
}

// The picture at pixel `pos`: `picture` is asked at its centre, 0..1 across and down, for a colour
// straight, and it is kept premultiplied. A pixel that ran out of its budget, or came back with what
// isn't a colour, is clear.
@fragment
fn host_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    v = HostValues(/*READ*/);
    let made = picture(pos.xy / host_u.size.xy);
    if (host_budget < 0 || !all(abs(made) < vec4<f32>(3.0e38))) {
        return vec4<f32>(0.0);
    }
    let shown = clamp(made, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec4<f32>(shown.rgb * shown.a, shown.a);
}

// A sample spread over the picture that says only whether a pixel ran out: a picture that never
// ends is found here, before it is drawn whole.
@fragment
fn host_probe(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    v = HostValues(/*READ*/);
    let made = picture(pos.xy / 16.0);
    return vec4<f32>(select(0.0, 1.0, host_budget < 0), made.a * 0.0, 0.0, 1.0);
}
