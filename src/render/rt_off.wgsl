// Stand-ins for the ray-traced lighting when the GPU can't trace rays: the shadow maps do all
// the work and reflections show the sky.

const RAY_TRACING: bool = false;

fn traced_sun(p: vec3<f32>, n: vec3<f32>, cheap: bool) -> f32 {
    return 1.0;
}

fn traced_occlusion(p: vec3<f32>, n: vec3<f32>, cheap: bool) -> f32 {
    return 1.0;
}

fn traced_reflection(p: vec3<f32>, dir: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(0.0);
}

fn traced_irradiance(p: vec3<f32>, n: vec3<f32>, cheap: bool) -> vec3<f32> {
    return sky_irradiance(n);
}

fn contact_occlusion(p: vec3<f32>, n: vec3<f32>, pixel: vec2<f32>) -> f32 {
    return 1.0;
}
