// Standard meshes. Prepended with pbr.wgsl.

@group(1) @binding(0) var base_color_texture: texture_2d<f32>;
@group(1) @binding(1) var normal_texture: texture_2d<f32>;
@group(1) @binding(2) var metallic_roughness_texture: texture_2d<f32>;
@group(1) @binding(3) var material_sampler: sampler;
@group(1) @binding(4) var height_texture: texture_2d<f32>;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(11) color: vec3<f32>,
};

struct InstanceInput {
    @location(3) model_0: vec4<f32>,
    @location(4) model_1: vec4<f32>,
    @location(5) model_2: vec4<f32>,
    @location(6) model_3: vec4<f32>,
    @location(7) normal_0: vec3<f32>,
    @location(8) normal_1: vec3<f32>,
    @location(9) normal_2: vec3<f32>,
    @location(10) color: vec4<f32>,
    // x: roughness, y: metallic, z: normal map strength, w: alpha cutoff (0 = opaque)
    @location(12) params: vec4<f32>,
    // rgb: emitted light, w: translucency
    @location(13) emissive: vec4<f32>,
    // x: parallax height in metres (0 = none), y: subsurface scattering, z: weathering,
    // w: 1 for a decal
    @location(14) extra: vec4<f32>,
    // x: puddles, y: animated waves, zw: flow of the water over the ground, metres a second
    @location(15) more: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) @interpolate(flat) params: vec4<f32>,
    @location(5) @interpolate(flat) emissive: vec4<f32>,
    @location(6) @interpolate(flat) extra: vec4<f32>,
    @location(7) @interpolate(flat) more: vec4<f32>,
};

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    let model = mat4x4<f32>(instance.model_0, instance.model_1, instance.model_2, instance.model_3);
    let normal_matrix = mat3x3<f32>(instance.normal_0, instance.normal_1, instance.normal_2);
    let world_position = model * vec4<f32>(vertex.position, 1.0);

    var out: VertexOutput;
    out.clip_position = view.view_proj * world_position;
    out.world_position = world_position.xyz;
    out.world_normal = normal_matrix * vertex.normal;
    out.uv = vertex.uv;
    out.color = instance.color * vec4<f32>(vertex.color, 1.0);
    out.params = instance.params;
    out.emissive = instance.emissive;
    out.extra = instance.extra;
    out.more = instance.more;
    return out;
}

// Relief from a height map: where the view ray meets the surface, the tangent frame, and the
// scale, for the parallax and its shadows.
struct Relief {
    uv: vec2<f32>,
    // Depth below the top of the relief at the point seen, 0 to 1.
    depth: f32,
    t: vec3<f32>,
    b: vec3<f32>,
    // Texture units per unit of depth.
    depth_uv: f32,
    found: bool,
};

// Parallax occlusion mapping: march the view ray down through the height field and return
// where it first passes below the surface. Stones and mortar joints then shift and hide one
// another as the eye moves, as real ones do.
fn parallax(uv: vec2<f32>, n: vec3<f32>, p: vec3<f32>, height_m: f32, ddx_uv: vec2<f32>, ddy_uv: vec2<f32>) -> Relief {
    var relief = Relief(uv, 0.0, vec3<f32>(0.0), vec3<f32>(0.0), 0.0, false);
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    // With screen y running down, the frame comes out mirrored; put its sign back (see
    // perturb_normal).
    let handed = select(-1.0, 1.0, dot(dp1, dp2perp) >= 0.0);
    let t = (dp2perp * ddx_uv.x + dp1perp * ddy_uv.x) * handed;
    let b = (dp2perp * ddx_uv.y + dp1perp * ddy_uv.y) * handed;
    let tl = length(t);
    let bl = length(b);
    if tl < 1e-12 || bl < 1e-12 || height_m <= 0.0 {
        return relief;
    }
    // Metres of surface per unit of texture coordinate, so the height is in true scale.
    let metres_per_uv = length(dp1) / max(length(ddx_uv), 1e-8);
    let v = normalize(view.camera_position.xyz - p);
    let v_ts = vec3<f32>(dot(v, t / tl), dot(v, b / bl), dot(v, n));
    let distance = length(view.camera_position.xyz - p);
    // Nearby, march finely; far off the effect is too small to see.
    let fade = 1.0 - smoothstep(18.0, 45.0, distance);
    if fade <= 0.0 {
        return relief;
    }
    // At grazing angles the march steps through the height field too coarsely and breaks up
    // into scales; flatten the relief there, where it is foreshortened to nothing anyway.
    let grazing = smoothstep(0.12, 0.45, abs(v_ts.z));
    let depth_uv = height_m / metres_per_uv * fade * grazing;
    let steps = mix(48.0, 12.0, abs(v_ts.z));
    let step_uv = -v_ts.xy / max(v_ts.z, 0.12) * depth_uv / steps;
    var current = uv;
    var layer = 0.0;
    let layer_step = 1.0 / steps;
    var h = 1.0 - textureSampleGrad(height_texture, material_sampler, current, ddx_uv, ddy_uv).r;
    var previous_h = h;
    var previous_uv = current;
    for (var i = 0; i < 64; i++) {
        if layer >= h || f32(i) >= steps {
            break;
        }
        previous_uv = current;
        previous_h = h;
        current += step_uv;
        layer += layer_step;
        h = 1.0 - textureSampleGrad(height_texture, material_sampler, current, ddx_uv, ddy_uv).r;
    }
    // Interpolate between the last two steps for a smooth result.
    let after = h - layer;
    let before = previous_h - (layer - layer_step);
    let w = clamp(after / (after - before + 1e-5), 0.0, 1.0);
    relief.uv = mix(current, previous_uv, w);
    relief.depth = mix(layer, layer - layer_step, w);
    relief.t = t / tl;
    relief.b = b / bl;
    relief.depth_uv = depth_uv;
    relief.found = true;
    return relief;
}

// Shadows the relief casts on itself: from the point seen, march up toward the sun through the
// height field; stones standing in the way shade the joints and the sides of their neighbours.
// Soft, by how far below the stones the ray passes.
fn relief_shadow(relief: Relief, n: vec3<f32>, ddx_uv: vec2<f32>, ddy_uv: vec2<f32>) -> f32 {
    if !relief.found {
        return 1.0;
    }
    let l = view.sun_direction.xyz;
    let l_ts = vec3<f32>(dot(l, relief.t), dot(l, relief.b), dot(l, n));
    if l_ts.z <= 0.02 {
        return 1.0;
    }
    let steps = 16.0;
    let rise = relief.depth / steps;
    let step_uv = l_ts.xy / l_ts.z * relief.depth_uv * rise;
    var uv = relief.uv;
    var ray = relief.depth;
    var blocked = 0.0;
    for (var i = 0; i < 16; i++) {
        uv += step_uv;
        ray -= rise;
        if ray <= 0.0 {
            break;
        }
        let surface = 1.0 - textureSampleGrad(height_texture, material_sampler, uv, ddx_uv, ddy_uv).r;
        // Positive where the height field stands above the ray.
        blocked = max(blocked, (ray - surface) * (1.0 - f32(i) / steps));
    }
    return 1.0 - smoothstep(0.0, 0.08, blocked);
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(in.world_normal);
    if !front {
        n = -n;
    }
    // Derivatives first, while control flow is uniform.
    let ddx_uv = dpdx(in.uv);
    let ddy_uv = dpdy(in.uv);
    let relief = parallax(in.uv, n, in.world_position, in.extra.x, ddx_uv, ddy_uv);
    let uv = relief.uv;
    let self_shadow = relief_shadow(relief, n, ddx_uv, ddy_uv);

    let base = textureSampleGrad(base_color_texture, material_sampler, uv, ddx_uv, ddy_uv) * in.color;
    let cutoff = in.params.w;
    // Sharpen alpha around the cutoff so alpha-to-coverage gives crisp, smooth edges.
    let coverage = saturate((base.a - cutoff) / max(fwidth(base.a), 1e-4) + 0.5);
    let alpha = select(1.0, coverage, cutoff > 0.0);

    // A strength of zero leaves the normal as it is.
    let tangent_normal = unpack_normal(textureSampleGrad(normal_texture, material_sampler, uv, ddx_uv, ddy_uv).xyz, in.params.z);
    n = perturb_normal(n, in.world_position, in.uv, tangent_normal);
    let mr = textureSampleGrad(metallic_roughness_texture, material_sampler, uv, ddx_uv, ddy_uv);

    let geometric = select(-1.0, 1.0, front) * normalize(in.world_normal);
    let decal = in.extra.w > 0.5;
    // Contact shadows, and the grime that collects in the same hemmed-in places. Decals lie on
    // a surface that already has them.
    let open = select(contact_occlusion(in.world_position, geometric, in.clip_position.xy), 1.0, decal);
    var albedo = base.rgb * weathering(in.world_position, normalize(in.world_normal), in.extra.z) * crevice_grime(open, in.extra.z);
    var roughness = clamp(in.params.x * mr.g, 0.03, 1.0);
    // Dirt packed into the low parts of the relief: the joints between setts and stones, the
    // hollows of worn plaster. Dark, brown, and rougher.
    if relief.found && in.extra.z > 0.0 {
        let low = 1.0 - smoothstep(0.08, 0.5, 1.0 - relief.depth);
        let packed = low * saturate(in.extra.z);
        albedo *= mix(vec3<f32>(1.0), vec3<f32>(0.36, 0.3, 0.23), packed);
        roughness = mix(roughness, 1.0, packed * 0.6);
    }

    // Puddles on level ground after rain. Water gathers first in the joints and hollows (where
    // the height map is low) of wider basins in the paving; around it the stone is dark and damp.
    // The water's surface is flat and still, a mirror for the traced reflections.
    let puddles = in.more.x;
    if puddles > 0.0 && geometric.y > 0.8 {
        let p = in.world_position;
        let basin = noise3d(vec3<f32>(p.x * 0.16, 0.0, p.z * 0.16)) * 0.65 + noise3d(vec3<f32>(p.x * 0.55, 3.0, p.z * 0.55)) * 0.35;
        let height = textureSampleGrad(height_texture, material_sampler, uv, ddx_uv, ddy_uv).r;
        let level = (basin - 0.55) * 2.2 + puddles * 0.55;
        let water = smoothstep(-0.02, 0.03, level - height);
        let damp = smoothstep(-0.1, 0.0, level - height);
        albedo *= mix(1.0, 0.5, damp) * mix(1.0, 0.7, water);
        roughness = mix(roughness, roughness * 0.35, damp);
        roughness = mix(roughness, 0.015, water);
        // Still water, but not dead: a breeze stirs faint ripples across it.
        n = normalize(mix(n, water_normal(p, geometric, 0.35, vec2<f32>(0.25, -0.1)), water));
    }
    // Open water: the river flowing, the basin stirred by its jets, the jets themselves.
    if in.more.y > 0.0 {
        n = water_normal(in.world_position, geometric, in.more.y, in.more.zw);
    }

    var s = default_surface(albedo, n);
    s.roughness = roughness;
    s.metallic = saturate(in.params.y * mr.b);
    // The joints and hollows of the relief are shut in: they see little sky.
    let cavity = select(1.0, 1.0 - relief.depth * 0.7, relief.found);
    s.occlusion = mr.r * mix(0.12, 1.0, open) * cavity;
    s.translucency = in.emissive.w;
    s.emissive = in.emissive.rgb;
    s.subsurface = in.extra.y;
    s.geometric = geometric;
    s.sun_occlusion = self_shadow;
    let color = apply_haze(shade(s, in.world_position), in.world_position);
    if decal {
        // Decals blend over what is beneath by their own alpha.
        return vec4<f32>(color, base.a);
    }
    if alpha <= 0.0 {
        discard;
    }
    return vec4<f32>(color, alpha);
}
