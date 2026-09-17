struct View {
    view_proj: mat4x4<f32>,
    camera_position: vec4<f32>,
    // Direction the light travels, in world space.
    light_direction: vec4<f32>,
    light_color: vec4<f32>,
    ambient_color: vec4<f32>,
};

@group(0) @binding(0)
var<uniform> view: View;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
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
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) uv: vec2<f32>,
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
    out.color = instance.color;
    out.uv = vertex.uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = -normalize(view.light_direction.xyz);
    let v = normalize(view.camera_position.xyz - in.world_position);
    let h = normalize(l + v);

    let diffuse = max(dot(n, l), 0.0) * view.light_color.rgb;
    let specular = pow(max(dot(n, h), 0.0), 32.0) * 0.2 * view.light_color.rgb;

    // Faint checker from the UVs so flat surfaces have some texture while moving around.
    let checker = select(0.92, 1.0, (i32(floor(in.uv.x * 8.0)) + i32(floor(in.uv.y * 8.0))) % 2 == 0);
    let base = in.color.rgb * checker;

    return vec4<f32>(base * (view.ambient_color.rgb + diffuse) + specular, in.color.a);
}
