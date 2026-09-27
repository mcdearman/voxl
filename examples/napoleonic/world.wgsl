// The ground and everything built on it, in photographic materials. Prepended with
// render::PBR_WGSL.
//
// Terrain vertices carry blend weights for the six ground layers; other vertices name one
// layer. UVs are in metres; each layer says how many metres one repeat of its texture covers.

@group(1) @binding(0) var albedo_array: texture_2d_array<f32>;
// rgb: tangent-space normal, a: roughness
@group(1) @binding(1) var normal_array: texture_2d_array<f32>;
@group(1) @binding(2) var array_sampler: sampler;
// Per layer. x: metres per repeat, y: roughness scale, z: normal strength, w: 1 to hide tiling
@group(1) @binding(3) var<uniform> layers: array<vec4<f32>, 16>;

const GROUND_LAYERS: u32 = 6u;
const SINGLE: u32 = 255u;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    // Ground blend weights: grass, straw, soil, road, forest floor, mud.
    @location(3) weights_a: vec4<f32>,
    @location(4) weights_b: vec4<f32>,
    // rgb: colour multiplier (1 is neutral, up to 2), a: extra occlusion
    @location(5) tint: vec4<f32>,
    // Which layer, or 255 for a ground blend.
    @location(6) layer: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) weights_a: vec4<f32>,
    @location(4) weights_b: vec4<f32>,
    @location(5) tint: vec4<f32>,
    @location(6) @interpolate(flat) layer: u32,
};

@vertex
fn vs_main(v: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = view.view_proj * vec4<f32>(v.position, 1.0);
    out.world_position = v.position;
    out.normal = v.normal;
    out.uv = v.uv;
    out.weights_a = v.weights_a;
    out.weights_b = v.weights_b;
    out.tint = vec4<f32>(v.tint.rgb * 2.0, v.tint.a);
    out.layer = v.layer;
    return out;
}

// ---- Noise for breaking up repetition ------------------------------------------------------

fn hash2(p: vec2<f32>) -> f32 {
    let q = vec2<u32>(vec2<i32>(floor(p)) + vec2<i32>(32768));
    var h = q.x * 0x85ebca6bu ^ (q.y * 0xc2b2ae35u);
    h ^= h >> 15u;
    h *= 0x2c1b3c6du;
    h ^= h >> 12u;
    return f32(h) / 4294967295.0;
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash2(i), hash2(i + vec2<f32>(1.0, 0.0)), u.x),
        mix(hash2(i + vec2<f32>(0.0, 1.0)), hash2(i + vec2<f32>(1.0, 1.0)), u.x),
        u.y,
    );
}

fn fbm(p: vec2<f32>) -> f32 {
    return value_noise(p) * 0.5 + value_noise(p * 2.03 + 7.1) * 0.3 + value_noise(p * 4.1 + 3.3) * 0.2;
}

// ---- Layer sampling -------------------------------------------------------------------------

struct Sample {
    albedo: vec3<f32>,
    // Tangent space.
    normal: vec3<f32>,
    roughness: f32,
};

fn sample_at(layer: u32, uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>) -> Sample {
    let a = textureSampleGrad(albedo_array, array_sampler, uv, layer, ddx, ddy);
    let n = textureSampleGrad(normal_array, array_sampler, uv, layer, ddx, ddy);
    // OpenGL-convention normals: flip green, as texture V runs down the image.
    return Sample(a.rgb, (n.rgb * 2.0 - 1.0) * vec3<f32>(1.0, -1.0, 1.0), n.a);
}

// One layer at a point. Tiling layers are sampled twice, at rotated and scaled coordinates,
// and mixed by slow noise, so no repeat is ever seen twice in a row.
fn sample_layer(layer: u32, uv_metres: vec2<f32>, ddx_m: vec2<f32>, ddy_m: vec2<f32>) -> Sample {
    let p = layers[layer];
    let scale = 1.0 / p.x;
    var s = sample_at(layer, uv_metres * scale, ddx_m * scale, ddy_m * scale);
    if p.w > 0.5 {
        let rot = mat2x2<f32>(0.8, 0.6, -0.6, 0.8);
        let scale2 = scale * 0.37;
        let uv2 = rot * uv_metres * scale2 + vec2<f32>(0.31, 0.77);
        let s2 = sample_at(layer, uv2, rot * ddx_m * scale2, rot * ddy_m * scale2);
        let t = smoothstep(0.35, 0.65, fbm(uv_metres * 0.07));
        s.albedo = mix(s.albedo, s2.albedo, t);
        s.normal = mix(s.normal, s2.normal, t);
        s.roughness = mix(s.roughness, s2.roughness, t);
    }
    s.normal = vec3<f32>(s.normal.xy * p.z, max(s.normal.z, 0.2));
    s.roughness = saturate(s.roughness * p.y);
    return s;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Colour shifts over tens of metres, as soil, moisture and growth vary across a real field.
fn macro_variation(p: vec2<f32>) -> vec3<f32> {
    let n1 = fbm(p * 0.013);
    let n2 = fbm(p * 0.05 + 11.0);
    let warm = vec3<f32>(1.06, 1.0, 0.88);
    let cool = vec3<f32>(0.9, 1.0, 1.02);
    return mix(cool, warm, n1) * (0.82 + 0.36 * n2);
}

// Terrain: blend the ground layers by weight, letting each texture's bright, raised parts
// poke through its neighbours so transitions follow pebbles and tufts rather than straight fades.
// ---- Fields ---------------------------------------------------------------------------------
//
// The same layout as terrain.rs, worked out per pixel so field edges stay crisp at any
// distance. `lattice_hash` matches the engine's value-noise lattice hash bit for bit.

const FIELD_ANGLE: f32 = 0.35;
const FIELD_WIDTH: f32 = 85.0;
const FIELD_LENGTH: f32 = 70.0;
const HEADLAND: f32 = 1.8;

fn lattice_hash(seed: u32, x: f32, y: f32) -> f32 {
    var h = seed ^ (bitcast<u32>(i32(x)) * 0x85ebca6bu) ^ (bitcast<u32>(i32(y)) * 0xc2b2ae35u);
    h ^= h >> 15u;
    h *= 0x2c1b3c6du;
    h ^= h >> 12u;
    h *= 0x297a2d39u;
    h ^= h >> 15u;
    return f32(h) / 4294967295.0;
}

struct Field {
    // grass, straw, soil
    weights: vec3<f32>,
    tint: vec3<f32>,
};

fn field_at(p: vec2<f32>) -> Field {
    let s = sin(FIELD_ANGLE);
    let c = cos(FIELD_ANGLE);
    let f = vec2<f32>(p.x * c + p.y * s, -p.x * s + p.y * c);
    let column = floor(f.x / FIELD_WIDTH);
    let offset = lattice_hash(5u, column, 0.0) * FIELD_LENGTH;
    let row = floor((f.y + offset) / FIELD_LENGTH);
    let along = f.x - column * FIELD_WIDTH;
    let down = f.y + offset - row * FIELD_LENGTH;
    let edge = min(min(along, FIELD_WIDTH - along), min(down, FIELD_LENGTH - down));
    let r = lattice_hash(8u, column, row);
    var out = Field(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0));
    if r < 0.28 {
        out.weights = vec3<f32>(0.0, 0.8, 0.2);
    } else if r < 0.40 {
        out = Field(vec3<f32>(0.0, 0.9, 0.1), vec3<f32>(1.1, 1.08, 1.02));
    } else if r < 0.50 {
        out.weights = vec3<f32>(0.5, 0.5, 0.0);
    } else if r < 0.70 {
        out.weights = vec3<f32>(1.0, 0.0, 0.0);
    } else if r < 0.82 {
        out = Field(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.88, 0.95, 0.85));
    } else if r < 0.91 {
        out.weights = vec3<f32>(0.0, 0.0, 1.0);
    } else {
        out = Field(vec3<f32>(0.0, 0.6, 0.4), vec3<f32>(1.08, 1.02, 0.9));
    }
    // A strip of grass where the plough turned, ragged along its inner edge.
    let ragged = HEADLAND + (value_noise(p * 0.8) - 0.5) * 0.9;
    if edge < ragged {
        out.weights = vec3<f32>(1.0, 0.0, 0.0);
    }
    out.tint *= 0.93 + 0.14 * lattice_hash(12u, column, row);
    return out;
}

// Terrain: blend the ground layers by weight, letting each texture's bright, raised parts
// poke through its neighbours so transitions follow pebbles and tufts rather than straight fades.
fn ground(in: VertexOutput, ddx_uv: vec2<f32>, ddy_uv: vec2<f32>) -> Sample {
    let uv = in.world_position.xz;
    // Far away, sample blurrier mips: the texture's detail averages out instead of repeating.
    let distance = length(in.world_position - view.camera_position.xyz);
    let blur = 1.0 + smoothstep(30.0, 400.0, distance) * 8.0;
    let ddx_m = ddx_uv * blur;
    let ddy_m = ddy_uv * blur;
    var w = array<f32, 6>(in.weights_a.x, in.weights_a.y, in.weights_a.z, in.weights_a.w, in.weights_b.x, in.weights_b.y);
    let farmland = in.weights_b.z;
    var tint = vec3<f32>(1.0);
    if farmland > 0.01 {
        let field = field_at(uv);
        w[0] += field.weights.x * farmland;
        w[1] += field.weights.y * farmland;
        w[2] += field.weights.z * farmland;
        tint = mix(vec3<f32>(1.0), field.tint, farmland);
    }
    var samples: array<Sample, 6>;
    var best = 0.0;
    for (var i = 0u; i < GROUND_LAYERS; i++) {
        if w[i] > 0.02 {
            samples[i] = sample_layer(i, uv, ddx_m, ddy_m);
            w[i] = w[i] * (0.4 + luminance(samples[i].albedo) * 2.5);
            best = max(best, w[i]);
        }
    }
    var out = Sample(vec3<f32>(0.0), vec3<f32>(0.0), 0.0);
    var total = 0.0;
    for (var i = 0u; i < GROUND_LAYERS; i++) {
        if w[i] > 0.02 {
            let sharp = max(w[i] - best * 0.6, 0.0);
            out.albedo += samples[i].albedo * sharp;
            out.normal += samples[i].normal * sharp;
            out.roughness += samples[i].roughness * sharp;
            total += sharp;
        }
    }
    out.albedo /= max(total, 1e-4);
    out.normal /= max(total, 1e-4);
    out.roughness /= max(total, 1e-4);
    out.albedo *= macro_variation(uv) * tint;
    return out;
}

// ---- Shading --------------------------------------------------------------------------------

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    var s: Sample;
    // Derivatives first: they need uniform control flow, which the branch below isn't.
    let uv = select(in.uv, in.world_position.xz, in.layer == SINGLE);
    let ddx_uv = dpdx(uv);
    let ddy_uv = dpdy(uv);
    if in.layer == SINGLE {
        s = ground(in, ddx_uv, ddy_uv);
    } else {
        s = sample_layer(in.layer, in.uv, ddx_uv, ddy_uv);
        s.albedo *= mix(vec3<f32>(1.0), macro_variation(in.world_position.xz * 3.0), 0.3);
    }
    let normal = perturb_normal(n, in.world_position, uv, normalize(s.normal));

    var surface = default_surface(s.albedo * in.tint.rgb, normal);
    surface.roughness = clamp(s.roughness, 0.04, 1.0);
    surface.occlusion = 1.0 - in.tint.a;
    let color = shade(surface, in.world_position);
    return vec4<f32>(apply_haze(color, in.world_position), 1.0);
}

// ---- Water ----------------------------------------------------------------------------------

// A slow, murky river: the sky mirrored in a surface ruffled by the breeze.
@fragment
fn fs_water(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.world_position.xz;
    let t = time();
    // Two layers of ripples drifting downstream and with the wind.
    let e = 0.15;
    var slope = vec2<f32>(0.0);
    for (var i = 0; i < 4; i++) {
        let f = 0.35 * pow(2.1, f32(i));
        let dir = vec2<f32>(cos(f32(i) * 1.7), sin(f32(i) * 1.7));
        let q = p * f + dir * t * (0.6 + f32(i) * 0.3);
        let dx = value_noise(q + vec2<f32>(e, 0.0)) - value_noise(q - vec2<f32>(e, 0.0));
        let dy = value_noise(q + vec2<f32>(0.0, e)) - value_noise(q - vec2<f32>(0.0, e));
        slope += vec2<f32>(dx, dy) / (2.0 * e) * 0.06 / f32(i + 1);
    }
    let n = normalize(vec3<f32>(-slope.x, 1.0, -slope.y));
    let v = normalize(view.camera_position.xyz - in.world_position);
    let fresnel = 0.02 + 0.98 * pow(1.0 - saturate(dot(n, v)), 5.0);
    let r = reflect(-v, n);
    let traced = traced_reflection(in.world_position + vec3<f32>(0.0, 0.05, 0.0), r);
    let reflected = mix(sample_sky(r, 0.5), traced.rgb, traced.w);
    // Silty water scatters a little of the daylight back.
    let body = vec3<f32>(0.035, 0.045, 0.035) * sky_irradiance(vec3<f32>(0.0, 1.0, 0.0));
    let l = view.sun_direction.xyz;
    let hv = normalize(l + v);
    let glint = pow(saturate(dot(n, hv)), 900.0) * 300.0 * view.sun_color.rgb * sun_visibility(in.world_position, n);
    let color = mix(body, reflected, fresnel) + glint * fresnel;
    return vec4<f32>(apply_haze(color, in.world_position), 1.0);
}
