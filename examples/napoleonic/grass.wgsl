// Grass, cereal and wildflowers, grown on the GPU around the camera. Prepended with
// render::PBR_WGSL.
//
// Each instance is one blade or stalk, drawn as a 9-vertex strip. Instances come in tiles of
// the ground: the instance index says which tile (from the draw's first instance) and which
// blade within it.

struct Tile {
    // xy: world x/z of the tile's corner, z: fraction of blades drawn, w: width boost
    data: vec4<f32>,
};

struct GrassParams {
    // x: tile size, y: max blades per tile, z: grid step, w: grid samples per side
    grid: vec4<f32>,
    // xy: grid origin (world x/z), z: fade start, w: fade end
    extent: vec4<f32>,
    // xy: wind direction, z: wind strength
    wind: vec4<f32>,
};

@group(1) @binding(0) var heights: texture_2d<f32>;
// r: grass density, g: cereal density, b: dryness, a: grass height
@group(1) @binding(1) var cover: texture_2d<f32>;
@group(1) @binding(2) var cover_sampler: sampler;
@group(1) @binding(3) var<storage, read> tiles: array<Tile>;
@group(1) @binding(4) var<uniform> params: GrassParams;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    // x: height along the blade (0 root, 1 tip), y: translucency, z: roughness
    @location(3) info: vec3<f32>,
};

fn hash1(n: u32) -> f32 {
    var h = n * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    h = (h >> 22u) ^ h;
    return f32(h) / 4294967295.0;
}

// The terrain height at a point, interpolated exactly as the terrain mesh's triangles are.
fn ground_height(p: vec2<f32>) -> f32 {
    let g = (p - params.extent.xy) / params.grid.z;
    let n = i32(params.grid.w) - 2;
    let cell = clamp(vec2<i32>(floor(g)), vec2<i32>(0), vec2<i32>(n));
    let f = clamp(g - vec2<f32>(cell), vec2<f32>(0.0), vec2<f32>(1.0));
    let a = textureLoad(heights, cell, 0).r;
    let b = textureLoad(heights, cell + vec2<i32>(1, 0), 0).r;
    let c = textureLoad(heights, cell + vec2<i32>(0, 1), 0).r;
    let d = textureLoad(heights, cell + vec2<i32>(1, 1), 0).r;
    if f.x + f.y < 1.0 {
        return a + (b - a) * f.x + (c - a) * f.y;
    }
    return d + (c - d) * (1.0 - f.x) + (b - d) * (1.0 - f.y);
}

fn gust(p: vec2<f32>, t: f32) -> f32 {
    let along = dot(p, params.wind.xy);
    let wave = sin(along * 0.22 - t * 1.9 + sin(p.x * 0.05 + p.y * 0.07) * 2.0);
    let ripple = sin(along * 0.9 - t * 4.3 + p.y * 0.3) * 0.25;
    return (wave * 0.5 + 0.5) * 0.8 + ripple * 0.2;
}

fn collapsed() -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
    return out;
}

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOutput {
    let per_tile = u32(params.grid.y);
    let tile = tiles[instance / per_tile];
    let blade = instance % per_tile;
    if f32(blade) >= tile.data.z * params.grid.y {
        return collapsed();
    }
    let seed = blade * 7919u + u32(tile.data.x * 13.0 + 100000.0) * 104729u + u32(tile.data.y * 17.0 + 100000.0);
    let r1 = hash1(seed);
    let r2 = hash1(seed + 1u);
    let r3 = hash1(seed + 2u);
    let r4 = hash1(seed + 3u);
    // Blades grow in tufts: a dozen share a clump centre and splay out from it.
    let clump_seed = (blade / 12u) * 7919u + u32(tile.data.x * 13.0 + 100000.0) * 104729u + u32(tile.data.y * 17.0 + 100000.0);
    let clump = tile.data.xy + vec2<f32>(hash1(clump_seed + 11u), hash1(clump_seed + 12u)) * params.grid.x;
    let spread_angle = r1 * 6.2831;
    let spread = sqrt(r2) * 0.16;
    let root2 = clump + vec2<f32>(cos(spread_angle), sin(spread_angle)) * spread;

    let uv = (root2 - params.extent.xy) / (params.grid.z * (params.grid.w - 1.0));
    let c = textureSampleLevel(cover, cover_sampler, uv, 0.0);
    // Decide what grows here: cereal, grass, or nothing.
    let total = c.r + c.g;
    if r3 > total {
        return collapsed();
    }
    let cereal = r3 < c.g;
    let distance = length(root2 - view.camera_position.xz);
    let fade = 1.0 - smoothstep(params.extent.z, params.extent.w, distance);
    if fade <= 0.0 {
        return collapsed();
    }
    let kind_roll = hash1(seed + 4u);
    // In cereal, a few poppies and cornflowers; in grass, a few wildflowers.
    let flower = select(kind_roll < 0.012, kind_roll < 0.02, cereal);
    // Among live grass, some old blades have died and fallen over.
    let dead = !cereal && !flower && kind_roll > 0.86;

    var height: f32;
    var width: f32;
    if cereal {
        height = mix(0.85, 1.1, r4);
        width = 0.016;
        if flower {
            height *= 0.6;
        }
    } else {
        height = mix(0.18, 0.7, r4 * r4) * (0.4 + c.a);
        width = mix(0.012, 0.026, r1);
        if flower {
            height *= 1.1;
        }
    }
    height *= fade;
    width *= tile.data.w;

    // Blade shape: level along the strip, and which side.
    let level_index = min(vertex / 2u, 4u);
    let levels = array<f32, 5>(0.0, 0.35, 0.68, 0.88, 1.0);
    let t = levels[level_index];
    let side = select(-1.0, 1.0, vertex % 2u == 1u);
    var w = width * (1.0 - t * 0.85);
    if cereal && !flower {
        // A thin stem, then the ear.
        w = select(width * 0.45, width * 1.6, t > 0.78 && t < 0.99);
    }
    if flower && t > 0.8 {
        w = width * 2.2;
    }
    if vertex == 8u {
        w = 0.0;
    }

    // Each blade faces out from its tuft's centre, give or take, and leans that way.
    let outward = vec2<f32>(cos(spread_angle), sin(spread_angle));
    let yaw = spread_angle + (r3 - 0.5) * 1.2;
    let facing = vec2<f32>(cos(yaw), sin(yaw));
    let across = vec3<f32>(facing.y, 0.0, -facing.x);
    let lean_dir = normalize(vec3<f32>(outward.x, 0.0, outward.y) * 0.7 + vec3<f32>(facing.x, 0.0, facing.y) * 0.3);
    let time_now = time();
    let g = gust(root2, time_now);
    let wind = vec3<f32>(params.wind.x, 0.0, params.wind.y) * params.wind.z;
    let stiffness = select(1.0, 0.7, cereal);
    let droop = select(0.15 + r3 * 0.25 + spread * 1.5, 1.4, dead);
    let bend = lean_dir * droop * select(1.0, 0.25, cereal)
        + wind * (0.2 + g * 1.0) * stiffness
        + wind * sin(time_now * (5.0 + r2 * 4.0) + r1 * 30.0) * 0.08 * (0.5 + g);
    let root = vec3<f32>(root2.x, ground_height(root2), root2.y);
    let offset = bend * t * t * height;
    var p = root + vec3<f32>(0.0, t * height, 0.0) + offset + across * side * w * 0.5;
    // Keep the blade's length roughly constant as it bends.
    p.y -= length(offset) * t * 0.5;

    // A soft normal, bent toward the shape of the tuft (up, and out from its centre), so each
    // clump shades like one rounded mass rather than a spray of flat cards.
    let tangent = normalize(vec3<f32>(0.0, height, 0.0) + bend * 2.0 * t * height);
    let blade_normal = normalize(cross(across, tangent));
    let tuft_normal = normalize(vec3<f32>(outward.x * spread * 4.0, 1.0, outward.y * spread * 4.0));
    let n = normalize(mix(blade_normal, tuft_normal, 0.6));

    // Colours: greens vary blade to blade and dry toward straw; cereal is ripe gold.
    var base: vec3<f32>;
    var tip: vec3<f32>;
    if cereal {
        base = mix(vec3<f32>(0.22, 0.2, 0.07), vec3<f32>(0.28, 0.22, 0.08), r1);
        tip = mix(vec3<f32>(0.5, 0.34, 0.1), vec3<f32>(0.58, 0.42, 0.15), r2) * (0.85 + 0.3 * c.b);
    } else {
        let green = mix(vec3<f32>(0.06, 0.11, 0.025), vec3<f32>(0.12, 0.16, 0.04), r1);
        let dry = mix(vec3<f32>(0.26, 0.22, 0.1), vec3<f32>(0.4, 0.34, 0.17), r2);
        base = mix(green * 0.7, dry * 0.7, c.b);
        tip = mix(green * 1.5 + vec3<f32>(0.03, 0.03, 0.0), dry * 1.2, c.b);
    }
    var color = mix(base, tip, t);
    // Patches across the field: here greener and lusher, there paler and yellowing, over a few
    // metres and over tens of metres.
    let lushness = noise3d(vec3<f32>(root2.x * 0.09, 0.0, root2.y * 0.09)) * 0.6 + noise3d(vec3<f32>(root2.x * 0.4, 5.0, root2.y * 0.4)) * 0.4;
    let yellowing = smoothstep(0.55, 0.8, noise3d(vec3<f32>(root2.x * 0.05, 9.0, root2.y * 0.05)));
    color *= 0.75 + 0.5 * lushness;
    color = mix(color, color * vec3<f32>(1.35, 1.15, 0.6), yellowing * 0.6 * select(1.0, 0.2, cereal));
    if dead {
        color = mix(vec3<f32>(0.2, 0.16, 0.08), vec3<f32>(0.34, 0.28, 0.15), r1) * (0.6 + 0.4 * t);
    }
    if flower && t > 0.8 {
        let pick = hash1(seed + 5u);
        if cereal {
            // Poppies, and now and then a cornflower.
            color = select(vec3<f32>(0.55, 0.02, 0.01), vec3<f32>(0.06, 0.12, 0.6), pick < 0.2);
        } else {
            color = select(vec3<f32>(0.5, 0.48, 0.42), vec3<f32>(0.5, 0.38, 0.02), pick < 0.4);
        }
    }

    var out: VertexOutput;
    out.clip_position = view.view_proj * vec4<f32>(p, 1.0);
    out.world_position = p;
    out.normal = n;
    out.color = color;
    out.info = vec3<f32>(t, select(0.5, 0.35, cereal), select(0.8, 0.65, cereal));
    return out;
}

@fragment
fn fs_main(in: VertexOutput, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = normalize(in.normal);
    // Seen from behind, a blade faces the other way across, but still up with its tuft.
    if !front {
        n = normalize(vec3<f32>(-n.x, n.y, -n.z));
    }
    // Blades lower down are shaded by the ones around them.
    let occlusion = mix(0.5, 1.0, smoothstep(0.0, 0.8, in.info.x));
    var s = default_surface(in.color, n);
    s.roughness = in.info.z;
    s.occlusion = occlusion;
    s.translucency = in.info.y;
    s.cheap = true;
    var color = shade(s, in.world_position);
    // Direct sun is also blocked near the roots.
    color *= mix(0.7, 1.0, occlusion);
    return vec4<f32>(apply_haze(color, in.world_position), 1.0);
}
