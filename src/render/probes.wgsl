
// ---- Light probe bake. Prepended with pbr.wgsl and rt_on.wgsl. -----------------------------

struct Bake {
    // xyz: the first probe, w: metres between probes
    origin: vec4<f32>,
    // xyz: probe counts, w: rays per probe
    dims: vec4<u32>,
    // x: 0 for the first bounce (hit surfaces lit by the sky), 1 to light them by the probes;
    // y: the first slice along z this dispatch bakes
    bounce: vec4<u32>,
};

@group(1) @binding(0) var<uniform> bake: Bake;
@group(1) @binding(1) var out_mean: texture_storage_3d<rgba16float, write>;
@group(1) @binding(2) var out_x: texture_storage_3d<rgba16float, write>;
@group(1) @binding(3) var out_y: texture_storage_3d<rgba16float, write>;
@group(1) @binding(4) var out_z: texture_storage_3d<rgba16float, write>;

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

@compute @workgroup_size(4, 4, 4)
fn bake_probes(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let id = invocation + vec3<u32>(0u, 0u, bake.bounce.y);
    if any(id >= bake.dims.xyz) {
        return;
    }
    let p = bake.origin.xyz + vec3<f32>(id) * bake.origin.w;
    let rays = bake.dims.w;
    // Each probe turns its ray pattern a different way, so neighbours' patterns don't line up
    // into streaks.
    let h = pcg(id.x + pcg(id.y + pcg(id.z)));
    let spin = f32(h & 0xffffu) / 65536.0 * 6.2831853;
    let lean = (f32(h >> 16u) / 65536.0 - 0.5) / f32(rays);

    var mean = vec3<f32>(0.0);
    var gx = vec3<f32>(0.0);
    var gy = vec3<f32>(0.0);
    var gz = vec3<f32>(0.0);
    var backfaces = 0.0;
    for (var i = 0u; i < rays; i++) {
        // Evenly over the sphere: a spherical Fibonacci spiral.
        let y = clamp(1.0 - (2.0 * f32(i) + 1.0) / f32(rays) + lean, -1.0, 1.0);
        let r = sqrt(1.0 - y * y);
        let a = f32(i) * 2.39996323 + spin;
        let dir = vec3<f32>(cos(a) * r, y, sin(a) * r);

        var rq: ray_query;
        rayQueryInitialize(&rq, scene, RayDesc(0u, 0xffu, 0.0, 400.0, p, dir));
        rayQueryProceed(&rq);
        let hit = rayQueryGetCommittedIntersection(&rq);
        var light: vec3<f32>;
        if hit.kind == RAY_QUERY_INTERSECTION_NONE {
            light = sample_sky(dir, view.fog.w - 3.0);
        } else {
            // Seeing the back of a surface means being inside something.
            if dot(hit_surface(hit).normal, dir) > 0.0 {
                backfaces += 1.0;
            }
            light = shade_hit(hit, p, dir, 6.0, bake.bounce.x == 1u);
        }
        mean += light;
        gx += light * dir.x;
        gy += light * dir.y;
        gz += light * dir.z;
    }
    let n = f32(rays);
    // Radiance ≈ mean + lean · direction, the lean measured as 3/N Σ L·dir.
    let k = 3.0 / n;
    let validity = 1.0 - smoothstep(0.08, 0.2, backfaces / n);
    textureStore(out_mean, id, vec4<f32>(mean / n, validity));
    textureStore(out_x, id, vec4<f32>(gx * k, 0.0));
    textureStore(out_y, id, vec4<f32>(gy * k, 0.0));
    textureStore(out_z, id, vec4<f32>(gz * k, 0.0));
}
