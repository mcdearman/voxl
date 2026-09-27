// Voxel chunks. Prepended with render::PBR_WGSL.

@group(1) @binding(0)
var block_textures: texture_2d_array<f32>;
@group(1) @binding(1)
var block_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // face: bits 0-2, ambient occlusion: bits 3-4, texture layer: the rest
    @location(2) packed: u32,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) occlusion: f32,
    @location(2) world_position: vec3<f32>,
    @location(3) @interpolate(flat) packed: u32,
};

// Same order as `FACES` in mesher.rs.
const NORMALS = array<vec3<f32>, 6>(
    vec3<f32>(1.0, 0.0, 0.0),
    vec3<f32>(-1.0, 0.0, 0.0),
    vec3<f32>(0.0, 1.0, 0.0),
    vec3<f32>(0.0, -1.0, 0.0),
    vec3<f32>(0.0, 0.0, 1.0),
    vec3<f32>(0.0, 0.0, -1.0),
);

const OCCLUSION = array<f32, 4>(0.35, 0.6, 0.8, 1.0);

@vertex
fn vs_main(vertex: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = view.view_proj * vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.occlusion = OCCLUSION[(vertex.packed >> 3u) & 3u];
    out.world_position = vertex.position;
    out.packed = vertex.packed;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let normal = NORMALS[in.packed & 7u];
    let layer = in.packed >> 5u;
    let base = textureSample(block_textures, block_sampler, in.uv, layer).rgb;
    var s = default_surface(base, normal);
    s.roughness = 0.9;
    s.occlusion = in.occlusion;
    let color = apply_haze(shade(s, in.world_position), in.world_position);
    return vec4<f32>(color, 1.0);
}

