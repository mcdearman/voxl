// Bloom: the image is filtered down a chain of half-size textures, then back up, each step
// adding its blurrier light to the level above (Jimenez, "Next Generation Post Processing in
// Call of Duty: Advanced Warfare").

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOutput;
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, linear_sampler, uv, 0.0).rgb;
}

// 13 taps in overlapping 2x2 boxes.
fn downsample(uv: vec2<f32>) -> vec3<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(source));
    let a = tap(uv + t * vec2<f32>(-2.0, -2.0));
    let b = tap(uv + t * vec2<f32>(0.0, -2.0));
    let c = tap(uv + t * vec2<f32>(2.0, -2.0));
    let d = tap(uv + t * vec2<f32>(-2.0, 0.0));
    let e = tap(uv);
    let f = tap(uv + t * vec2<f32>(2.0, 0.0));
    let g = tap(uv + t * vec2<f32>(-2.0, 2.0));
    let h = tap(uv + t * vec2<f32>(0.0, 2.0));
    let i = tap(uv + t * vec2<f32>(2.0, 2.0));
    let j = tap(uv + t * vec2<f32>(-1.0, -1.0));
    let k = tap(uv + t * vec2<f32>(1.0, -1.0));
    let l = tap(uv + t * vec2<f32>(-1.0, 1.0));
    let m = tap(uv + t * vec2<f32>(1.0, 1.0));
    return e * 0.125 + (a + c + g + i) * 0.03125 + (b + d + f + h) * 0.0625 + (j + k + l + m) * 0.125;
}

@fragment
fn fs_downsample(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(downsample(in.uv), 1.0);
}

// The first step also tames single blazing pixels (the sun glinting off a bayonet) so they
// don't flicker as big blobs.
@fragment
fn fs_downsample_first(in: VertexOutput) -> @location(0) vec4<f32> {
    let c = downsample(in.uv);
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    return vec4<f32>(c / (1.0 + luma * 0.02), 1.0);
}

// A 3x3 tent, added onto the larger level by the pipeline's blending.
@fragment
fn fs_upsample(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(source));
    var sum = tap(uv_offset(in.uv, t, -1.0, -1.0)) + tap(uv_offset(in.uv, t, 1.0, -1.0))
        + tap(uv_offset(in.uv, t, -1.0, 1.0)) + tap(uv_offset(in.uv, t, 1.0, 1.0));
    sum += (tap(uv_offset(in.uv, t, 0.0, -1.0)) + tap(uv_offset(in.uv, t, -1.0, 0.0))
        + tap(uv_offset(in.uv, t, 1.0, 0.0)) + tap(uv_offset(in.uv, t, 0.0, 1.0))) * 2.0;
    sum += tap(in.uv) * 4.0;
    return vec4<f32>(sum / 16.0, 1.0);
}

fn uv_offset(uv: vec2<f32>, t: vec2<f32>, x: f32, y: f32) -> vec2<f32> {
    return uv + t * vec2<f32>(x, y);
}
