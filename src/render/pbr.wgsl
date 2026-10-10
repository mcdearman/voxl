// Shared by every pipeline that draws into the main pass: the view, the sky, the sun and its
// shadows, and physically based shading. Custom shaders are prepended with this file (see
// `render::PBR_WGSL`), so everything below is available to them at group 0.

const PI: f32 = 3.14159265;
const CASCADES: u32 = 4u;

struct View {
    view_proj: mat4x4<f32>,
    inverse_view_proj: mat4x4<f32>,
    // w: seconds since startup
    camera_position: vec4<f32>,
    // w: exposure
    camera_forward: vec4<f32>,
    // Toward the sun. w: sky rotation in radians
    sun_direction: vec4<f32>,
    // Sun illuminance, linear RGB. w: sky intensity
    sun_color: vec4<f32>,
    // Flat extra ambient light, linear RGB. w: distance at which the haze begins
    ambient: vec4<f32>,
    // x: haze density at base height, y: height falloff, z: base height, w: sky mip count
    fog: vec4<f32>,
    // Sky irradiance / π as order-2 spherical harmonics.
    sh: array<vec4<f32>, 9>,
    cascades: array<mat4x4<f32>, 4>,
    // Far distance of each cascade along the view direction.
    cascade_splits: vec4<f32>,
    // World size of a shadow texel in each cascade.
    cascade_texels: vec4<f32>,
    // x: shadow map texel size in UV, y: 1 if the sun casts shadows, z: the view mode,
    // w: shadow fade start distance
    shadow_params: vec4<f32>,
    // xy: framebuffer size, zw: 1 / size
    viewport: vec4<f32>,
    // Light probes. xyz: the first probe, w: metres between probes
    probe_origin: vec4<f32>,
    // xyz: probe counts, w: 1 once they hold a finished bake
    probe_dims: vec4<f32>,
    // x: frame number (mod 1024), y: 1 when frames are blended over time (TAA), zw: jitter
    temporal: vec4<f32>,
};

// A storage buffer rather than a uniform: the same group may hold a texture binding array.
@group(0) @binding(0) var<storage, read> view: View;
@group(0) @binding(1) var shadow_map: texture_depth_2d_array;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
@group(0) @binding(3) var sky_texture: texture_2d<f32>;
@group(0) @binding(4) var sky_sampler: sampler;

fn saturate(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}

fn luminance3(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn time() -> f32 {
    return view.camera_position.w;
}

// ---- Sky ------------------------------------------------------------------------------------

fn sky_uv(direction: vec3<f32>) -> vec2<f32> {
    // Undo the sky's rotation about Y, then map to equirectangular coordinates.
    let a = -view.sun_direction.w;
    let d = vec3<f32>(
        cos(a) * direction.x + sin(a) * direction.z,
        direction.y,
        -sin(a) * direction.x + cos(a) * direction.z,
    );
    return vec2<f32>(0.5 + atan2(d.x, -d.z) / (2.0 * PI), acos(clamp(d.y, -1.0, 1.0)) / PI);
}

fn sample_sky(direction: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(sky_texture, sky_sampler, sky_uv(direction), lod).rgb * view.sun_color.w;
}

fn sky_irradiance(n: vec3<f32>) -> vec3<f32> {
    let s = view.sh;
    let r = s[0].rgb * 0.282095
        + s[1].rgb * 0.488603 * n.y
        + s[2].rgb * 0.488603 * n.z
        + s[3].rgb * 0.488603 * n.x
        + s[4].rgb * 1.092548 * n.x * n.y
        + s[5].rgb * 1.092548 * n.y * n.z
        + s[6].rgb * 0.315392 * (3.0 * n.z * n.z - 1.0)
        + s[7].rgb * 1.092548 * n.x * n.z
        + s[8].rgb * 0.546274 * (n.x * n.x - n.y * n.y);
    return max(r, vec3<f32>(0.0)) * view.sun_color.w + view.ambient.rgb;
}

// ---- Shadows --------------------------------------------------------------------------------

fn view_depth(world_position: vec3<f32>) -> f32 {
    return dot(world_position - view.camera_position.xyz, view.camera_forward.xyz);
}

fn sample_cascade(cascade: u32, world_position: vec3<f32>) -> f32 {
    let clip = view.cascades[cascade] * vec4<f32>(world_position, 1.0);
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z > 1.0 {
        return 1.0;
    }
    // A 4x4 tent of hardware-filtered taps for soft, stable edges.
    let texel = view.shadow_params.x;
    var lit = 0.0;
    for (var y = -1.5; y <= 1.5; y += 1.0) {
        for (var x = -1.5; x <= 1.5; x += 1.0) {
            lit += textureSampleCompareLevel(
                shadow_map, shadow_sampler, uv + vec2<f32>(x, y) * texel, cascade, ndc.z,
            );
        }
    }
    return lit / 16.0;
}

// How much sunlight reaches this point, 0 to 1.
fn sun_visibility(world_position: vec3<f32>, normal: vec3<f32>) -> f32 {
    if view.shadow_params.y < 0.5 {
        return 1.0;
    }
    let depth = view_depth(world_position);
    var cascade = CASCADES;
    for (var i = 0u; i < CASCADES; i++) {
        if depth < view.cascade_splits[i] {
            cascade = i;
            break;
        }
    }
    if cascade == CASCADES {
        return 1.0;
    }
    // Push the lookup off the surface in proportion to the cascade's texel size, so surfaces
    // don't shadow themselves.
    let n_dot_l = dot(normal, view.sun_direction.xyz);
    let texel_world = view.cascade_texels[cascade];
    let offset_dir = normal * (1.5 - saturate(n_dot_l) * 0.75);
    var lit = sample_cascade(cascade, world_position + offset_dir * texel_world);
    // Blend into the next cascade near the boundary to hide the seam.
    let split = view.cascade_splits[cascade];
    let blend = saturate((depth - split * 0.9) / (split * 0.1));
    if blend > 0.0 && cascade + 1u < CASCADES {
        let next = sample_cascade(cascade + 1u, world_position + offset_dir * view.cascade_texels[cascade + 1u]);
        lit = mix(lit, next, blend);
    }
    let fade = saturate((depth - view.shadow_params.w) / (view.cascade_splits[CASCADES - 1u] - view.shadow_params.w));
    return mix(lit, 1.0, fade);
}

// ---- Surfaces -------------------------------------------------------------------------------

struct Surface {
    albedo: vec3<f32>,
    // World space, unit length, facing the viewer.
    normal: vec3<f32>,
    roughness: f32,
    metallic: f32,
    // Ambient occlusion from maps or geometry, 0 to 1.
    occlusion: f32,
    // Light passing through thin things like leaves and grass, 0 to 1.
    translucency: f32,
    emissive: vec3<f32>,
    // Trace fewer rays: for masses of small things like grass blades.
    cheap: bool,
    // Light scattering beneath the surface, 0 to 1, as in skin.
    subsurface: f32,
    // The surface's own smooth normal, before any normal map: rays for the light from all
    // around leave along it, so the result doesn't flicker with every bump in the texture.
    geometric: vec3<f32>,
    // How much of the sun reaches past the surface's own relief (parallax shadows).
    sun_occlusion: f32,
};

fn default_surface(albedo: vec3<f32>, normal: vec3<f32>) -> Surface {
    return Surface(albedo, normal, 0.8, 0.0, 1.0, 0.0, vec3<f32>(0.0), false, 0.0, normal, 1.0);
}

fn d_ggx(n_dot_h: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d);
}

fn v_smith(n_dot_v: f32, n_dot_l: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
    let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-5);
}

fn fresnel(f0: vec3<f32>, cos_theta: f32) -> vec3<f32> {
    return f0 + (1.0 - f0) * pow(1.0 - cos_theta, 5.0);
}

// Karis' analytic fit of the split-sum environment BRDF.
fn env_brdf(f0: vec3<f32>, roughness: f32, n_dot_v: f32) -> vec3<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * n_dot_v)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

// Sun, sky and reflections on a surface, before haze.
fn shade(surface: Surface, world_position: vec3<f32>) -> vec3<f32> {
    var s = surface;
    // View modes, for looking into a scene: 1 the surface's own colour, 3 its normal, both
    // set against the exposure so they come out as they are; 2 the light on plain grey.
    let mode = view.shadow_params.z;
    if mode == 1.0 {
        return (s.albedo + s.emissive) / max(view.camera_forward.w, 1e-4);
    }
    if mode == 3.0 {
        return (s.normal * 0.5 + 0.5) / max(view.camera_forward.w, 1e-4);
    }
    if mode == 2.0 {
        s.albedo = vec3<f32>(0.5);
        s.metallic = 0.0;
        s.emissive = vec3<f32>(0.0);
    }
    let v = normalize(view.camera_position.xyz - world_position);
    let n = s.normal;
    let l = view.sun_direction.xyz;
    let h = normalize(l + v);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_l = saturate(dot(n, l));
    let a = max(s.roughness * s.roughness, 0.002);
    let f0 = mix(vec3<f32>(0.04), s.albedo, s.metallic);
    let diffuse_color = s.albedo * (1.0 - s.metallic);

    // The shadow maps carry what ray queries can't (cut-out foliage); rays carry the rest.
    let shadow = sun_visibility(world_position, n) * traced_sun(world_position, n, s.cheap) * s.sun_occlusion;
    let f = fresnel(f0, saturate(dot(h, v)));
    var specular = d_ggx(saturate(dot(n, h)), a) * v_smith(n_dot_v, n_dot_l, a) * f;
    // Light entering skin travels a few millimetres before leaving: the shadow side of a face
    // glows warm where light wraps round, and the terminator is soft and reddish rather than
    // hard. Wrap lighting, with the extra light tinted by blood.
    let raw = dot(n, l);
    let wrap = s.subsurface * 0.5;
    let wrapped = saturate((raw + wrap) / (1.0 + wrap));
    let scatter = max(wrapped - n_dot_l, 0.0) * vec3<f32>(1.0, 0.32, 0.18) * s.subsurface;
    if s.subsurface > 0.0 {
        // Skin has two sheens: the oily surface layer, and a broader one beneath.
        let a2 = max(a * 0.45, 0.002);
        specular = specular * 0.6 + d_ggx(saturate(dot(n, h)), a2) * v_smith(n_dot_v, n_dot_l, a2) * f * 0.4;
    }
    let direct = (diffuse_color / PI * (1.0 - f) + specular) * n_dot_l + diffuse_color / PI * scatter * 1.6;
    // Sunlight scattering through leaves and blades from behind.
    let back = saturate(dot(-n, l)) * 0.6 + pow(saturate(dot(-v, l)), 4.0) * 0.4;
    let transmitted = diffuse_color / PI * back * s.translucency;
    var color = (direct + transmitted) * view.sun_color.rgb * shadow;

    // Skylight, and the sky mirrored in the surface, blurrier the rougher it is.
    let brdf = env_brdf(f0, s.roughness, n_dot_v);
    let r = reflect(-v, n);
    let lod = s.roughness * (view.fog.w - 3.0);
    // Reflections of the ground below the horizon are darker than the sky they'd sample.
    let horizon = saturate(1.0 + dot(r, vec3<f32>(0.0, 1.0, 0.0)) * 3.0);
    // Light from all around: traced (sky and bounce off everything nearby) where the GPU can.
    // Traced about the smooth normal, then bent to the detailed one by how the sky lights each.
    let bend = sky_irradiance(n) / max(sky_irradiance(s.geometric), vec3<f32>(1e-4));
    let irradiance = traced_irradiance(world_position, s.geometric, s.cheap) * clamp(bend, vec3<f32>(0.5), vec3<f32>(2.0));
    // A blurry reflection sees about as much open sky as the surface does; where walls close
    // in, it reflects them instead: darker, and tinted like the light they send.
    let open_sky = luminance3(irradiance) / max(luminance3(sky_irradiance(n)), 1e-4);
    var mirrored = mix(irradiance, sample_sky(r, lod), saturate(open_sky)) * horizon;
    if RAY_TRACING && s.roughness < 0.35 {
        let traced = traced_reflection(world_position + n * 0.03, r);
        mirrored = mix(mirrored, select(sample_sky(r, lod) * horizon, traced.rgb, traced.w > 0.5), 1.0 - s.roughness / 0.35);
    }
    // Reflections off a bump that point back into the surface beneath it see only the stone
    // around, not the sky (horizon occlusion); and a crevice hides its reflections as it hides
    // its light (specular occlusion, Lagarde).
    let below = saturate(1.0 + 1.3 * dot(r, s.geometric));
    let specular_occlusion = saturate(pow(n_dot_v + s.occlusion, exp2(-16.0 * s.roughness - 1.0)) - 1.0 + s.occlusion);
    let reflected = mirrored * brdf * below * below * specular_occlusion;
    // Light in shade comes mostly from the open sky above: the tops of stones and mouldings
    // catch more of it than their undersides, relative to the surface they stand on.
    let skyward = clamp(1.0 + (n.y - s.geometric.y) * 0.7, 0.55, 1.35);
    let ambient = irradiance * diffuse_color * (1.0 - brdf) * skyward * s.occlusion + reflected;
    color += ambient;
    return color + s.emissive;
}

// ---- Haze -----------------------------------------------------------------------------------

// Aerial perspective: exponential height fog, lit by the sky and glowing toward the sun.
fn apply_haze(color: vec3<f32>, world_position: vec3<f32>) -> vec3<f32> {
    // The modes that show a surface as it is show it without the air in front of it.
    if view.shadow_params.z == 1.0 || view.shadow_params.z == 3.0 {
        return color;
    }
    let to = world_position - view.camera_position.xyz;
    let full_distance = length(to);
    let dir = to / max(full_distance, 1e-4);
    // Only the part of the ray beyond the haze's start passes through any.
    let distance = max(full_distance - view.ambient.w, 0.0);
    let density = view.fog.x;
    let falloff = view.fog.y;
    let height = view.camera_position.y - view.fog.z;
    let dy = dir.y * distance * falloff;
    // Integral of density along the ray through an exponential atmosphere.
    var optical = density * exp(-height * falloff) * distance;
    if abs(dy) > 1e-4 {
        optical *= (1.0 - exp(-dy)) / dy;
    }
    let extinction = 1.0 - exp(-optical);
    let horizon_dir = normalize(vec3<f32>(dir.x, max(dir.y, 0.03), dir.z));
    let sky = sample_sky(horizon_dir, view.fog.w - 4.0);
    let sun_glow = pow(saturate(dot(dir, view.sun_direction.xyz)), 8.0) * view.sun_color.rgb * 0.02;
    return mix(color, sky + sun_glow, extinction);
}

// ---- Normal maps ----------------------------------------------------------------------------

// Tangent-space normal mapping without stored tangents (Schüler, "Normal Mapping Without
// Precomputed Tangents"): the frame comes from screen-space derivatives of position and UV.
fn perturb_normal(n: vec3<f32>, world_position: vec3<f32>, uv: vec2<f32>, tangent_normal: vec3<f32>) -> vec3<f32> {
    let dp1 = dpdx(world_position);
    let dp2 = dpdy(world_position);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    // Schüler's solution leaves out a division by the frame's determinant, harmless in OpenGL
    // where screen y runs up. Here it runs down, which flips the determinant's sign and with it
    // both axes (normal maps lit from the wrong side, parallax marching backward), so the sign
    // is put back.
    let handed = select(-1.0, 1.0, dot(dp1, dp2perp) >= 0.0);
    let t = (dp2perp * duv1.x + dp1perp * duv2.x) * handed;
    let b = (dp2perp * duv1.y + dp1perp * duv2.y) * handed;
    let invmax = inverseSqrt(max(dot(t, t), dot(b, b)));
    if invmax > 1e20 {
        return n;
    }
    let tbn = mat3x3<f32>(t * invmax, b * invmax, n);
    return normalize(tbn * tangent_normal);
}

fn unpack_normal(sample: vec3<f32>, strength: f32) -> vec3<f32> {
    // OpenGL-convention maps (glTF, Poly Haven) point green up the image; texture V runs down.
    let xy = (sample.xy * 2.0 - 1.0) * vec2<f32>(1.0, -1.0) * strength;
    return vec3<f32>(xy, sqrt(saturate(1.0 - dot(xy, xy))));
}

// Screen-space dither for alpha-to-coverage and to break up banding.
fn interleaved_gradient_noise(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// ---- Weathering ------------------------------------------------------------------------------

fn hash31(p: vec3<f32>) -> f32 {
    let q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    let r = q + dot(q, q.yzx + 33.33);
    return fract((r.x + r.y) * r.z);
}

fn noise3d(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let n000 = hash31(i);
    let n100 = hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash31(i + vec3<f32>(1.0, 1.0, 1.0));
    return mix(
        mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y),
        mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y),
        u.z,
    );
}

// How years of weather have darkened a surface: soot and grime in blotches a metre or two
// across, rain streaks running down walls from every ledge, and mud splashed up from the
// street. Returns a colour multiplier; `amount` 0 leaves the surface as it is.
fn weathering(p: vec3<f32>, n: vec3<f32>, amount: f32) -> vec3<f32> {
    if amount <= 0.0 {
        return vec3<f32>(1.0);
    }
    let wall = 1.0 - abs(n.y);
    let blot = noise3d(p * 0.4) * 0.6 + noise3d(p * 1.7 + 11.0) * 0.4;
    let streak = noise3d(vec3<f32>(p.x * 5.0, p.y * 0.3, p.z * 5.0)) * 0.7 + noise3d(p * 3.1) * 0.3;
    let splash = 1.0 - smoothstep(0.0, 1.3, p.y);
    let soot = smoothstep(0.35, 0.8, blot) * 0.5;
    let rain = wall * smoothstep(0.45, 0.85, streak) * 0.45;
    let mud = wall * splash * (0.4 + 0.25 * noise3d(p * 6.0));
    let darkness = saturate((soot + rain + mud) * amount);
    // Soot is grey-black; mud a little warm.
    let tint = mix(vec3<f32>(0.62, 0.6, 0.58), vec3<f32>(0.55, 0.47, 0.38), splash);
    var color = mix(vec3<f32>(1.0), tint, darkness);
    // Rising damp: a dark, faintly green tide mark up the foot of walls, its top edge ragged.
    let tide = 0.55 + 0.35 * noise3d(vec3<f32>(p.x * 0.8, 0.0, p.z * 0.8));
    let damp = wall * (1.0 - smoothstep(tide - 0.15, tide, p.y)) * saturate(amount);
    color *= mix(vec3<f32>(1.0), vec3<f32>(0.62, 0.64, 0.55), damp * 0.8);
    // Variation over tens of metres, so no two stretches of the same stone or paving match
    // and the repeat of the texture disappears.
    let broad = noise3d(p * 0.045 + 3.0) * 0.6 + noise3d(p * 0.13 + 7.0) * 0.4;
    let hue = noise3d(p * 0.07 + 19.0) - 0.5;
    color *= (0.84 + 0.3 * broad) * vec3<f32>(1.0 + hue * 0.12, 1.0, 1.0 - hue * 0.14);
    return color;
}

// The moving surface of water: a normal wobbled by ripples that drift with the flow (`flow`,
// metres a second, across level water; falling water streams downward), with a few long swells
// under fine chop. `amount` 0 leaves the surface still.
fn water_normal(p: vec3<f32>, n: vec3<f32>, amount: f32, flow: vec2<f32>) -> vec3<f32> {
    if amount <= 0.0 {
        return n;
    }
    let t = view.camera_position.w;
    let level = n.y > 0.5;
    let drift = select(vec3<f32>(0.0, -2.2, 0.0), vec3<f32>(flow.x, 0.0, flow.y), level);
    // A height field of scrolling noise at two scales, differenced for its slope.
    let e = 0.02;
    let h = water_height(p, drift, t);
    let gx = water_height(p + vec3<f32>(e, 0.0, 0.0), drift, t) - h;
    let gy = water_height(p + vec3<f32>(0.0, e, 0.0), drift, t) - h;
    let gz = water_height(p + vec3<f32>(0.0, 0.0, e), drift, t) - h;
    var slope = vec3<f32>(gx, gy, gz) / e;
    // Only the slope along the surface tilts it.
    slope -= n * dot(slope, n);
    // Long swells rolling across level water.
    if level {
        let swell = cos(dot(p.xz, vec2<f32>(0.9, 0.4)) * 1.7 - t * 1.3) * vec2<f32>(0.9, 0.4) * 1.7
            + cos(dot(p.xz, vec2<f32>(-0.3, 0.95)) * 2.3 - t * 1.7) * vec2<f32>(-0.3, 0.95) * 2.3;
        slope += vec3<f32>(swell.x, 0.0, swell.y) * 0.004;
    }
    return normalize(n - slope * amount);
}

fn water_height(p: vec3<f32>, drift: vec3<f32>, t: f32) -> f32 {
    let q = p - drift * t;
    return noise3d(q * 3.1 + vec3<f32>(0.0, t * 0.6, 0.0)) * 0.012 + noise3d(q * 9.0 + vec3<f32>(t * 1.1, 0.0, -t * 0.7)) * 0.004;
}

// Dirt that gathers where a surface is hemmed in: in corners, under ledges and mouldings,
// between stones, where walls meet the ground. `open` is how much of the surroundings the
// surface sees (1 in the open).
fn crevice_grime(open: f32, amount: f32) -> vec3<f32> {
    let hemmed = saturate(1.0 - open) * saturate(amount);
    return mix(vec3<f32>(1.0), vec3<f32>(0.58, 0.54, 0.48), hemmed);
}
