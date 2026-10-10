// How far each moving thing has come across the picture since the last frame, for temporal
// anti-aliasing to look for it where it was. Only what moved is drawn here, over the depth
// the main pass left.

struct Motion {
    // This frame's camera as the main pass used it, sub-pixel nudge and all, so depths agree.
    view_proj: mat4x4<f32>,
    // The camera now and a frame ago, without the nudge.
    now: mat4x4<f32>,
    before: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform> motion: Motion;

struct Out {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) now: vec4<f32>,
    @location(1) before: vec4<f32>,
}

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(3) model_0: vec4<f32>,
    @location(4) model_1: vec4<f32>,
    @location(5) model_2: vec4<f32>,
    @location(6) model_3: vec4<f32>,
    @location(7) was_0: vec4<f32>,
    @location(8) was_1: vec4<f32>,
    @location(9) was_2: vec4<f32>,
    @location(10) was_3: vec4<f32>,
) -> Out {
    let model = mat4x4<f32>(model_0, model_1, model_2, model_3);
    let was = mat4x4<f32>(was_0, was_1, was_2, was_3);
    let world_position = model * vec4<f32>(position, 1.0);
    var out: Out;
    out.clip_position = motion.view_proj * world_position;
    out.now = motion.now * world_position;
    out.before = motion.before * (was * vec4<f32>(position, 1.0));
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    // In the units the picture is sampled in: across is 0 to 1, and down is positive.
    let moved = (in.now.xy / in.now.w - in.before.xy / in.before.w) * vec2<f32>(0.5, -0.5);
    return vec4<f32>(moved, 0.0, 1.0);
}
