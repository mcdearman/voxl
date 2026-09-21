struct View {
    view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    light_direction: vec4<f32>,
    light_color: vec4<f32>,
    ambient_color: vec4<f32>,
    fog_color: vec4<f32>,
    // x: start distance, y: end distance
    fog: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> view: View;

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

    let l = -normalize(view.light_direction.xyz);
    let diffuse = max(dot(normal, l), 0.0) * view.light_color.rgb;
    let lit = base * (view.ambient_color.rgb + diffuse) * in.occlusion;

    // Distance fog toward the sky colour hides chunks popping in at the edge of the world.
    let distance = length(view.camera_position.xyz - in.world_position);
    let fog = smoothstep(view.fog.x, view.fog.y, distance);
    return vec4<f32>(mix(lit, view.fog_color.rgb, fog), 1.0);
}
