// Turns the linear HDR image into what the display shows: exposure, bloom, a filmic curve,
// then dithering so smooth skies don't band.

struct Post {
    // x: exposure, y: bloom strength, z: vignette strength, w: saturation
    params: vec4<f32>,
    // x: contrast (1 = plain AgX), y: the look's extra saturation
    look: vec4<f32>,
    // Colour grading: tints multiplying the shadows and the highlights of the finished image.
    shadows: vec4<f32>,
    highlights: vec4<f32>,
    // x: sharpening, y: film grain, z: seconds (to move the grain), w: colour temperature
    extra: vec4<f32>,
};

@group(0) @binding(0) var hdr_texture: texture_2d<f32>;
@group(0) @binding(1) var bloom_texture: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;
@group(0) @binding(3) var<uniform> post: Post;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VertexOutput;
    out.clip_position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return out;
}

// AgX (Troy Sobotka), via Benjamin Wrensch's polynomial fit. Bright colours desaturate toward
// white the way film does, instead of clipping into flat primaries.
fn agx_contrast(x: vec3<f32>) -> vec3<f32> {
    let x2 = x * x;
    let x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4 - 6.868 * x2 * x + 0.4298 * x2
        + 0.1191 * x - 0.00232;
}

fn agx(color: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104),
    );
    let outset = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116),
    );
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var c = inset * max(color, vec3<f32>(1e-10));
    c = clamp(log2(c), vec3<f32>(min_ev), vec3<f32>(max_ev));
    c = (c - min_ev) / (max_ev - min_ev);
    c = agx_contrast(c);
    // The "look": plain AgX is deliberately flat, like log footage; a power curve gives it
    // the contrast and colour of a finished photograph.
    c = pow(max(c, vec3<f32>(0.0)), vec3<f32>(post.look.x));
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    c = luma + (c - luma) * post.look.y;
    // Grading: shadows and highlights tinted apart, as a colourist would.
    let tone = smoothstep(0.05, 0.85, dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)));
    c *= mix(post.shadows.rgb, post.highlights.rgb, tone);
    // Back to linear for the sRGB swapchain.
    return pow(max(outset * c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn squash(c: vec3<f32>) -> vec3<f32> {
    return c / (1.0 + max(c.r, max(c.g, c.b)));
}

fn unsquash(c: vec3<f32>) -> vec3<f32> {
    return c / max(1.0 - max(c.r, max(c.g, c.b)), 1e-4);
}

fn soft_load(p: vec2<i32>) -> vec3<f32> {
    let last = vec2<i32>(textureDimensions(hdr_texture)) - 1;
    return squash(textureLoad(hdr_texture, clamp(p, vec2<i32>(0), last), 0).rgb);
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sharpening against the four neighbours puts back the crispness that blending frames
    // over time takes off (done on softened values, so bright edges don't ring).
    let p = vec2<i32>(in.clip_position.xy);
    let middle = soft_load(p);
    let around = (soft_load(p + vec2<i32>(1, 0)) + soft_load(p - vec2<i32>(1, 0)) + soft_load(p + vec2<i32>(0, 1)) + soft_load(p - vec2<i32>(0, 1))) * 0.25;
    var color = unsquash(clamp(middle + (middle - around) * post.extra.x, vec3<f32>(0.0), vec3<f32>(0.999)));
    // White balance: warmer or cooler light.
    color *= vec3<f32>(1.0 + post.extra.w * 0.12, 1.0, 1.0 - post.extra.w * 0.12);
    let bloom = textureSampleLevel(bloom_texture, linear_sampler, in.uv, 0.0).rgb;
    color += bloom * post.params.y;
    color *= post.params.x;

    let luma = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    color = max(mix(vec3<f32>(luma), color, post.params.w), vec3<f32>(0.0));

    // A lens darkens toward the corners.
    let d = in.uv - 0.5;
    color *= 1.0 - dot(d, d) * post.params.z;

    var out = agx(color);
    // Half a step of noise hides 8-bit banding in the sky; film grain, if any, goes on top,
    // strongest in the mid-tones as on film.
    let noise = hash(in.clip_position.xy) - 0.5;
    let grain = hash(in.clip_position.xy + fract(post.extra.z * 7.13) * 1000.0) - 0.5;
    let mid = 1.0 - abs(dot(pow(out, vec3<f32>(1.0 / 2.2)), vec3<f32>(0.2126, 0.7152, 0.0722)) * 2.0 - 1.0);
    out = max(out + noise / 255.0 + grain * post.extra.y * mid * 0.1, vec3<f32>(0.0));
    return vec4<f32>(out, 1.0);
}
