// Standard meshes drawn into a shadow cascade.

@group(0) @binding(0) var<uniform> light_view_proj: mat4x4<f32>;

@group(1) @binding(0) var base_color_texture: texture_2d<f32>;
@group(1) @binding(3) var material_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct InstanceInput {
    @location(3) model_0: vec4<f32>,
    @location(4) model_1: vec4<f32>,
    @location(5) model_2: vec4<f32>,
    @location(6) model_3: vec4<f32>,
    @location(10) color: vec4<f32>,
    @location(12) params: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) cutoff: f32,
    @location(2) @interpolate(flat) alpha: f32,
};

@vertex
fn vs_main(vertex: VertexInput, instance: InstanceInput) -> VertexOutput {
    let model = mat4x4<f32>(instance.model_0, instance.model_1, instance.model_2, instance.model_3);
    var out: VertexOutput;
    out.clip_position = light_view_proj * model * vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.cutoff = instance.params.w;
    out.alpha = instance.color.a;
    return out;
}

// Only alpha-tested materials need a fragment stage: holes in leaves let light through.
@fragment
fn fs_mask(in: VertexOutput) {
    let alpha = textureSample(base_color_texture, material_sampler, in.uv).a * in.alpha;
    if alpha < in.cutoff {
        discard;
    }
}
