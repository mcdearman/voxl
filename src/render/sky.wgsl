// The sky behind everything. Prepended with pbr.wgsl.

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

// One triangle covering the screen, on the far plane (depth 0 with reversed Z), so anything
// drawn hides it.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    let ndc = uv * 2.0 - 1.0;
    var out: VertexOutput;
    out.clip_position = vec4<f32>(ndc, 0.0, 1.0);
    out.ndc = ndc;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Two points along the pixel's ray (reversed-Z puts depth 1 on the near plane) give its
    // direction whether the rays spread from the eye or run side by side.
    let near = view.inverse_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let beyond = view.inverse_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let dir = normalize(beyond.xyz / beyond.w - near.xyz / near.w);
    var color = sample_sky(dir, 0.0);
    // The sun was cut out of the sky picture to light the scene; paint its disk back.
    let sun_cos = dot(dir, view.sun_direction.xyz);
    let disk = smoothstep(0.999985, 0.999992, sun_cos);
    // Illuminance spread over the solid angle of the disk (0.27° radius).
    color += view.sun_color.rgb * disk / 6.8e-5;
    // The haze thickens toward the horizon, as it does over the land.
    let below = saturate(-dir.y * 8.0);
    color = mix(color, sample_sky(normalize(vec3<f32>(dir.x, 0.03, dir.z)), view.fog.w - 4.0), below);
    // The view modes that are readings of the scene want a plain ground to be read against.
    if view.shadow_params.z == 1.0 || view.shadow_params.z == 3.0 {
        color = vec3<f32>(0.03) / max(view.camera_forward.w, 1e-4);
    }
    return vec4<f32>(color, 1.0);
}
