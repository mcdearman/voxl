// Poses skinned vertices by their joints. See skin.rs.
//
// Vertices are the engine's `Vertex`: position, normal, uv, colour, eleven floats.

struct Params {
    vertices: u32,
    palette_offset: u32,
};

@group(0) @binding(0) var<storage, read> rest: array<f32>;
// Per vertex: joint indices (vec4<u32>), then weights as bits (vec4<u32>).
@group(0) @binding(1) var<storage, read> influences: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read> palette: array<mat4x4<f32>>;
@group(0) @binding(3) var<storage, read_write> posed: array<f32>;
@group(0) @binding(4) var<uniform> params: Params;

@compute @workgroup_size(64)
fn skin(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= params.vertices {
        return;
    }
    let b = i * 11u;
    let position = vec4<f32>(rest[b], rest[b + 1u], rest[b + 2u], 1.0);
    let normal = vec4<f32>(rest[b + 3u], rest[b + 4u], rest[b + 5u], 0.0);
    let joints = influences[i * 2u];
    let weights = bitcast<vec4<f32>>(influences[i * 2u + 1u]);
    let o = params.palette_offset;
    let m = palette[o + joints.x] * weights.x + palette[o + joints.y] * weights.y
        + palette[o + joints.z] * weights.z + palette[o + joints.w] * weights.w;
    let p = (m * position).xyz;
    let n = normalize((m * normal).xyz + vec3<f32>(0.0, 1e-6, 0.0));
    posed[b] = p.x;
    posed[b + 1u] = p.y;
    posed[b + 2u] = p.z;
    posed[b + 3u] = n.x;
    posed[b + 4u] = n.y;
    posed[b + 5u] = n.z;
    for (var k = 6u; k < 11u; k++) {
        posed[b + k] = rest[b + k];
    }
}
