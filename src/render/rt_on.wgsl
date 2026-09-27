// Ray-traced lighting, using hardware ray queries against the scene's acceleration structure.

const RAY_TRACING: bool = true;

@group(0) @binding(5) var scene: acceleration_structure;

fn occluded(origin: vec3<f32>, dir: vec3<f32>, tmax: f32) -> bool {
    var rq: ray_query;
    rayQueryInitialize(&rq, scene, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xffu, 0.0, tmax, origin, dir));
    rayQueryProceed(&rq);
    return rayQueryGetCommittedIntersection(&rq).kind != RAY_QUERY_INTERSECTION_NONE;
}

// A basis with `n` as its third axis.
fn basis(n: vec3<f32>) -> mat3x3<f32> {
    let up = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(n.y) > 0.99);
    let t = normalize(cross(up, n));
    return mat3x3<f32>(t, cross(n, t), n);
}

// Sunlight reaching a point: one ray straight at the sun. Without a denoiser, jittered rays
// sparkle; the sun is small enough that its shadows are nearly sharp anyway, and the shadow
// maps' filtering already softens their edges.
fn traced_sun(p: vec3<f32>, n: vec3<f32>, cheap: bool) -> f32 {
    // Grass and smoke are too many overlapping fragments to trace; the shadow maps serve them.
    if cheap {
        return 1.0;
    }
    let l = view.sun_direction.xyz;
    return select(1.0, 0.0, occluded(p + n * 0.02 + l * 0.01, l, 600.0));
}

// Ambient occlusion: how much of the sky a point sees within arm's reach, along five fixed
// directions (so it's steady from frame to frame) weighted by how directly they face out.
fn traced_occlusion(p: vec3<f32>, n: vec3<f32>, cheap: bool) -> f32 {
    if cheap {
        return 1.0;
    }
    let frame = basis(n);
    let origin = p + n * 0.02;
    let dirs = array<vec3<f32>, 5>(
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.75, 0.0, 0.66),
        vec3<f32>(-0.75, 0.0, 0.66),
        vec3<f32>(0.0, 0.75, 0.66),
        vec3<f32>(0.0, -0.75, 0.66),
    );
    var open = 0.0;
    var total = 0.0;
    for (var i = 0; i < 5; i++) {
        let w = dirs[i].z;
        open += select(w, 0.0, occluded(origin, frame * dirs[i], 1.0));
        total += w;
    }
    return mix(0.4, 1.0, open / total);
}

// Contact shadows: how open a point is within about a metre, from two short rays in random
// directions, different every pixel and frame; TAA averages them into smooth, detailed
// occlusion: dark under sills and ledges, where things stand on the ground, between cobbles.
// Nearer hits count for more. Without TAA it falls back to fixed directions.
fn contact_occlusion(p: vec3<f32>, n: vec3<f32>, pixel: vec2<f32>) -> f32 {
    if view.temporal.y < 0.5 {
        return traced_occlusion(p, n, false);
    }
    let frame = basis(n);
    let origin = p + n * 0.01;
    let reach = 0.9;
    let seed = pixel + view.temporal.x * vec2<f32>(5.588238, 3.3171);
    let a = fract(52.9829189 * fract(dot(seed, vec2<f32>(0.06711056, 0.00583715))));
    let b = fract(52.9829189 * fract(dot(seed.yx + 17.3, vec2<f32>(0.06711056, 0.00583715))));
    var open = 0.0;
    for (var i = 0; i < 2; i++) {
        let u = fract(a + f32(i) * 0.5);
        let phi = fract(b + f32(i) * 0.618034) * 6.2831853;
        let r = sqrt(u);
        let dir = frame * vec3<f32>(cos(phi) * r, sin(phi) * r, sqrt(1.0 - u));
        var rq: ray_query;
        rayQueryInitialize(&rq, scene, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xffu, 0.0, reach, origin, dir));
        rayQueryProceed(&rq);
        let hit = rayQueryGetCommittedIntersection(&rq);
        open += select(1.0, hit.t / reach, hit.kind != RAY_QUERY_INTERSECTION_NONE);
    }
    return open * 0.5;
}

// ---- Shading what a reflection ray hits ------------------------------------------------------

struct HitInstance {
    color: vec4<f32>,
    emissive: vec4<f32>,
    geometry: u32,
    texture: u32,
    roughness: f32,
    metallic: f32,
};

// Per vertex: normal xyz, then texture coordinate uv.
@group(0) @binding(6) var<storage, read> hit_vertices: array<f32>;
@group(0) @binding(7) var<storage, read> hit_indices: array<u32>;
// Per geometry: first vertex, first index.
@group(0) @binding(8) var<storage, read> hit_ranges: array<vec2<u32>>;
@group(0) @binding(9) var<storage, read> hit_instances: array<HitInstance>;
@group(0) @binding(10) var hit_textures: binding_array<texture_2d<f32>, 256>;
@group(0) @binding(11) var hit_sampler: sampler;

fn hit_vertex(i: u32) -> array<f32, 5> {
    let b = i * 5u;
    return array<f32, 5>(hit_vertices[b], hit_vertices[b + 1u], hit_vertices[b + 2u], hit_vertices[b + 3u], hit_vertices[b + 4u]);
}

struct HitSurface {
    // Interpolated from the vertices, in world space; it may face away from the ray.
    normal: vec3<f32>,
    uv: vec2<f32>,
    material: HitInstance,
};

fn hit_surface(hit: RayIntersection) -> HitSurface {
    let inst = hit_instances[hit.instance_custom_data];
    let range = hit_ranges[inst.geometry];
    let tri = range.y + hit.primitive_index * 3u;
    let a = hit_vertex(range.x + hit_indices[tri]);
    let b = hit_vertex(range.x + hit_indices[tri + 1u]);
    let c = hit_vertex(range.x + hit_indices[tri + 2u]);
    let w = vec3<f32>(1.0 - hit.barycentrics.x - hit.barycentrics.y, hit.barycentrics.x, hit.barycentrics.y);
    let n_obj = vec3<f32>(a[0], a[1], a[2]) * w.x + vec3<f32>(b[0], b[1], b[2]) * w.y + vec3<f32>(c[0], c[1], c[2]) * w.z;
    let uv = vec2<f32>(a[3], a[4]) * w.x + vec2<f32>(b[3], b[4]) * w.y + vec2<f32>(c[3], c[4]) * w.z;
    let n = normalize((hit.object_to_world * vec4<f32>(n_obj, 0.0)).xyz);
    return HitSurface(n, uv, inst);
}

// The light leaving a surface a ray has hit, lit by the sun (with a shadow ray) and by the
// light around it: the probes' when `bounce` is set, else the sky's, partly blocked. `lod`
// picks how blurry its texture is.
fn shade_hit(hit: RayIntersection, origin: vec3<f32>, dir: vec3<f32>, lod: f32, bounce: bool) -> vec3<f32> {
    let surface = hit_surface(hit);
    let inst = surface.material;
    var n = surface.normal;
    if dot(n, dir) > 0.0 {
        n = -n;
    }
    let texel = textureSampleLevel(hit_textures[inst.texture], hit_sampler, surface.uv, lod);
    let albedo = inst.color.rgb * texel.rgb * (1.0 - inst.metallic * 0.7);
    let at = origin + dir * hit.t;
    let l = view.sun_direction.xyz;
    let lit = select(0.0, 1.0, dot(n, l) > 0.0 && !occluded(at + n * 0.02, l, 600.0));
    let sun = view.sun_color.rgb * saturate(dot(n, l)) * lit / PI;
    var around = sky_irradiance(n) * 0.6;
    if bounce {
        around = probe_irradiance(at, n) + view.ambient.rgb;
    }
    return albedo * (sun + around) + inst.emissive.rgb;
}

// What a mirror ray sees: rgb, and in w whether it hit anything (else the caller uses the sky).
fn traced_reflection(p: vec3<f32>, dir: vec3<f32>) -> vec4<f32> {
    var rq: ray_query;
    rayQueryInitialize(&rq, scene, RayDesc(0u, 0xffu, 0.02, 1500.0, p, dir));
    rayQueryProceed(&rq);
    let hit = rayQueryGetCommittedIntersection(&rq);
    if hit.kind == RAY_QUERY_INTERSECTION_NONE {
        return vec4<f32>(0.0);
    }
    // Farther hits look at blurrier mips, much as the eye would.
    let lod = clamp(log2(hit.t + 1.0) * 1.2, 0.0, 11.0);
    let color = shade_hit(hit, p, dir, lod, view.probe_dims.w > 0.5);
    return vec4<f32>(apply_haze(color, p + dir * hit.t), 1.0);
}

// ---- Light probes (see probes.rs) ------------------------------------------------------------

// Per probe: the mean light arriving (w: 1 if the probe is in the open, 0 inside a wall), and
// how it leans along x, y and z.
@group(0) @binding(12) var probe_mean: texture_3d<f32>;
@group(0) @binding(13) var probe_x: texture_3d<f32>;
@group(0) @binding(14) var probe_y: texture_3d<f32>;
@group(0) @binding(15) var probe_z: texture_3d<f32>;

// The light arriving at a surface from everything around it, divided by π (so albedo times it
// is the light the surface gives back), blended from the eight probes around it.
fn probe_irradiance(p: vec3<f32>, n: vec3<f32>) -> vec3<f32> {
    let spacing = view.probe_origin.w;
    let dims = vec3<i32>(view.probe_dims.xyz);
    // Nudged off the surface, toward the probes in front of it.
    let g = (p + n * 0.3 * spacing - view.probe_origin.xyz) / spacing;
    if any(g < vec3<f32>(-0.5)) || any(g > vec3<f32>(dims) - 0.5) {
        return sky_irradiance(n) * 0.6;
    }
    let base = vec3<i32>(floor(g));
    let f = g - floor(g);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = 0; i < 8; i++) {
        let o = vec3<i32>(i & 1, (i >> 1) & 1, (i >> 2) & 1);
        let cell = clamp(base + o, vec3<i32>(0), dims - 1);
        let t = select(1.0 - f, f, o == vec3<i32>(1));
        var w = max(t.x * t.y * t.z, 1e-4);
        // Probes behind the surface see the wrong side of it.
        let to = view.probe_origin.xyz + vec3<f32>(cell) * spacing - p;
        let facing = (dot(to / max(length(to), 1e-4), n) + 1.0) * 0.5;
        w *= facing * facing + 0.02;
        let mean = textureLoad(probe_mean, cell, 0);
        w *= mean.w;
        // Irradiance / π of radiance mean + lean·ω is mean + ⅔ lean·n.
        let lean = textureLoad(probe_x, cell, 0).rgb * n.x
            + textureLoad(probe_y, cell, 0).rgb * n.y
            + textureLoad(probe_z, cell, 0).rgb * n.z;
        sum += max(mean.rgb + lean * (2.0 / 3.0), vec3<f32>(0.0)) * w;
        weight += w;
    }
    if weight < 1e-5 {
        return sky_irradiance(n) * 0.4;
    }
    return sum / weight;
}

fn traced_irradiance(p: vec3<f32>, n: vec3<f32>, cheap: bool) -> vec3<f32> {
    if view.probe_dims.w < 0.5 {
        return sky_irradiance(n);
    }
    return probe_irradiance(p, n) + view.ambient.rgb;
}
