
// ---- Light shafts: sunlight scattered along each view ray. Prepended with pbr.wgsl. ---------

struct Shafts {
    // x: density per metre, y: anisotropy, z: reach in metres, w: height falloff per metre
    params: vec4<f32>,
    // x: base height
    more: vec4<f32>,
};

@group(1) @binding(0) var scene_depth: texture_depth_multisampled_2d;
@group(1) @binding(1) var<uniform> shafts: Shafts;

@vertex
fn vs_shafts(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Sunlight at a point in the air: one tap of the cascade that covers it.
fn air_lit(p: vec3<f32>) -> f32 {
    let depth = view_depth(p);
    for (var i = 0u; i < CASCADES; i++) {
        if depth < view.cascade_splits[i] {
            let clip = view.cascades[i] * vec4<f32>(p, 1.0);
            let ndc = clip.xyz / clip.w;
            let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
            if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z > 1.0 {
                return 1.0;
            }
            return textureSampleCompareLevel(shadow_map, shadow_sampler, uv, i, ndc.z);
        }
    }
    return 1.0;
}

// Henyey-Greenstein: how much light scatters through the angle whose cosine is `c`.
fn phase(c: f32, g: f32) -> f32 {
    let d = 1.0 + g * g - 2.0 * g * c;
    return (1.0 - g * g) / (4.0 * PI * d * sqrt(d));
}

@fragment
fn fs_shafts(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    let depth = textureLoad(scene_depth, p, 0);
    let uv = position.xy * view.viewport.zw;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    let camera = view.camera_position.xyz;
    // A far point along the ray when it sees only sky (depth 0 is infinitely far).
    let far = view.inverse_view_proj * vec4<f32>(ndc, max(depth, 1e-7), 1.0);
    let end = far.xyz / far.w;
    let dir = normalize(end - camera);
    let distance = min(select(length(end - camera), 1e9, depth <= 0.0), shafts.params.z);

    let steps = 40u;
    let dt = distance / f32(steps);
    // A different starting offset per pixel and frame; TAA averages them out.
    let frame = view.temporal.x;
    let offset = fract(52.9829189 * fract(dot(position.xy + frame * 5.588238, vec2<f32>(0.06711056, 0.00583715))));
    var transmittance = 1.0;
    var scattered = 0.0;
    for (var i = 0u; i < steps; i++) {
        let x = camera + dir * ((f32(i) + offset) * dt);
        let density = shafts.params.x * exp(-max(x.y - shafts.more.x, 0.0) * shafts.params.w);
        scattered += transmittance * density * air_lit(x) * dt;
        transmittance *= exp(-density * dt);
    }
    let light = view.sun_color.rgb * phase(dot(dir, view.sun_direction.xyz), shafts.params.y) * scattered;
    return vec4<f32>(light, 0.0);
}
