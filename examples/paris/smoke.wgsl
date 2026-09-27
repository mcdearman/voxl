// Powder smoke, dust and woodsmoke: soft, lit, camera-facing billboards. Prepended with
// render::PBR_WGSL.

@group(1) @binding(0) var puffs: texture_2d_array<f32>;
@group(1) @binding(1) var puff_sampler: sampler;

struct Instance {
    // xyz: centre, w: radius
    @location(0) position: vec4<f32>,
    // rgb: albedo (or emitted light, for flashes), a: opacity
    @location(1) color: vec4<f32>,
    // x: rotation, y: texture layer, z: 1 for a flash, w: density toward the bottom
    @location(2) params: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) world_position: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) @interpolate(flat) params: vec4<f32>,
    @location(4) @interpolate(flat) center: vec3<f32>,
    @location(5) @interpolate(flat) radius: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, instance: Instance) -> VertexOutput {
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u)) * 2.0 - 1.0;
    let c = cos(instance.params.x);
    let s = sin(instance.params.x);
    let rotated = vec2<f32>(corner.x * c - corner.y * s, corner.x * s + corner.y * c);
    // Face the camera.
    let to_camera = normalize(view.camera_position.xyz - instance.position.xyz);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_camera));
    let up = cross(to_camera, right);
    let p = instance.position.xyz + (right * rotated.x + up * rotated.y) * instance.position.w;
    var out: VertexOutput;
    out.clip_position = view.view_proj * vec4<f32>(p, 1.0);
    out.uv = corner * 0.5 + 0.5;
    out.world_position = p;
    out.color = instance.color;
    out.params = instance.params;
    out.center = instance.position.xyz;
    out.radius = instance.position.w;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(puffs, puff_sampler, in.uv, u32(in.params.y));
    // Fade puffs that the camera is inside or right up against, rather than clipping them.
    let near = saturate((length(in.world_position - view.camera_position.xyz) - 0.3) / (in.radius * 1.5));
    let alpha = saturate(texel.a * in.color.a * near);
    if in.params.z > 0.5 {
        // A flash: pure emitted light, added.
        return vec4<f32>(in.color.rgb * texel.a * near, 0.0);
    }
    // Treat the puff as a ball: a normal from the billboard's UVs, lit from the sun's side.
    let q = in.uv * 2.0 - 1.0;
    let to_camera = normalize(view.camera_position.xyz - in.center);
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), to_camera));
    let up = cross(to_camera, right);
    let n = normalize(right * q.x + up * q.y + to_camera * sqrt(saturate(1.0 - dot(q, q))) * 0.7);
    let l = view.sun_direction.xyz;
    let v = to_camera;
    // Thick smoke is dark where it faces away from the sun, and glows looking into it.
    let wrap = saturate(dot(n, l) * 0.5 + 0.5);
    let forward = pow(saturate(dot(-v, l)), 6.0) * 1.5;
    let thickness = mix(1.0, 0.55, in.params.w * saturate(-q.y * 0.5 + 0.5));
    let shadow = sun_visibility(in.center, l) * traced_sun(in.center, l, true);
    let sun = view.sun_color.rgb * (wrap * 0.8 + forward) * shadow / PI;
    let sky = sky_irradiance(normalize(n + vec3<f32>(0.0, 0.6, 0.0)));
    var color = in.color.rgb * (sun + sky) * thickness;
    color = apply_haze(color, in.world_position);
    return vec4<f32>(color * alpha, alpha);
}
