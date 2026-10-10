// Temporal anti-aliasing: blends this frame into the re-projected history. See taa.rs.

struct Taa {
    // This frame's clip space to the last frame's, both without jitter.
    reproject: mat4x4<f32>,
    // x: 1 if the history is valid, y: weight of this frame, zw: 1 / size
    params: vec4<f32>,
    // x: 1 if things that moved wrote how far into `moved` this frame
    motion: vec4<f32>,
};

@group(0) @binding(0) var current: texture_2d<f32>;
@group(0) @binding(1) var history: texture_2d<f32>;
@group(0) @binding(2) var depth: texture_depth_multisampled_2d;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var<uniform> taa: Taa;
@group(0) @binding(5) var moved: texture_multisampled_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

// Colours are compared and blended after a soft tone curve, so a few very bright pixels can't
// dominate (and flicker), and in YCoCg, where a box around the neighbours fits them tightly.
fn squash(c: vec3<f32>) -> vec3<f32> {
    return c / (1.0 + max(c.r, max(c.g, c.b)));
}

fn unsquash(c: vec3<f32>) -> vec3<f32> {
    return c / max(1.0 - max(c.r, max(c.g, c.b)), 1e-4);
}

fn to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn from_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// The history sampled with a Catmull-Rom filter, sharper than bilinear, so repeated
// re-sampling doesn't blur the image (Jimenez's five-tap version).
fn sample_history(uv: vec2<f32>) -> vec3<f32> {
    let size = 1.0 / taa.params.zw;
    let pos = uv * size;
    let center = floor(pos - 0.5) + 0.5;
    let f = pos - center;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let tc0 = (center - 1.0) * taa.params.zw;
    let tc3 = (center + 2.0) * taa.params.zw;
    let tc12 = (center + w2 / w12) * taa.params.zw;
    var result = textureSampleLevel(history, linear_sampler, vec2<f32>(tc12.x, tc0.y), 0.0).rgb * (w12.x * w0.y);
    result += textureSampleLevel(history, linear_sampler, vec2<f32>(tc0.x, tc12.y), 0.0).rgb * (w0.x * w12.y);
    result += textureSampleLevel(history, linear_sampler, vec2<f32>(tc12.x, tc12.y), 0.0).rgb * (w12.x * w12.y);
    result += textureSampleLevel(history, linear_sampler, vec2<f32>(tc3.x, tc12.y), 0.0).rgb * (w3.x * w12.y);
    result += textureSampleLevel(history, linear_sampler, vec2<f32>(tc12.x, tc3.y), 0.0).rgb * (w12.x * w3.y);
    let weight = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;
    return max(result / weight, vec3<f32>(0.0));
}

// Pulls a colour toward the box's centre until it lies inside the box.
fn clip_to_box(color: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec3<f32> {
    let center = (lo + hi) * 0.5;
    let extent = max((hi - lo) * 0.5, vec3<f32>(1e-5));
    let offset = color - center;
    let t = abs(offset / extent);
    let most = max(t.x, max(t.y, t.z));
    return select(color, center + offset / most, most > 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(current));
    let p = vec2<i32>(position.xy);

    // The colours around this pixel, and the nearest surface among them: re-projecting by the
    // nearest keeps the edges of things in front sharp as the camera moves.
    var mean = vec3<f32>(0.0);
    var square = vec3<f32>(0.0);
    var lo = vec3<f32>(1e9);
    var hi = vec3<f32>(-1e9);
    var center = vec3<f32>(0.0);
    var nearest = 0.0;
    var nearest_at = p;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = clamp(p + vec2<i32>(dx, dy), vec2<i32>(0), size - 1);
            let c = to_ycocg(squash(textureLoad(current, q, 0).rgb));
            mean += c;
            square += c * c;
            lo = min(lo, c);
            hi = max(hi, c);
            if dx == 0 && dy == 0 {
                center = c;
            }
            // Reversed depth: nearer is greater.
            let d = textureLoad(depth, q, 0);
            if d > nearest {
                nearest = d;
                nearest_at = q;
            }
        }
    }
    let uv = (vec2<f32>(p) + 0.5) * taa.params.zw;
    let at = (vec2<f32>(nearest_at) + 0.5) * taa.params.zw;
    let ndc = vec4<f32>(at.x * 2.0 - 1.0, 1.0 - at.y * 2.0, nearest, 1.0);
    let previous = taa.reproject * ndc;
    var motion = vec2<f32>(previous.x / previous.w * 0.5 + 0.5, 0.5 - previous.y / previous.w * 0.5) - at;
    var behind = previous.w <= 0.0 && nearest > 0.0;
    // A thing that moved wrote how far it came; where nothing did, the target holds more than
    // anything could move.
    if taa.motion.x > 0.5 {
        let came = textureLoad(moved, nearest_at, 0).xy;
        if abs(came.x) < 2.0 {
            motion = -came;
            behind = false;
        }
    }
    let history_uv = uv + motion;

    let outside = any(history_uv < vec2<f32>(0.0)) || any(history_uv > vec2<f32>(1.0));
    if taa.params.x < 0.5 || outside || behind {
        return vec4<f32>(unsquash(from_ycocg(center)), 1.0);
    }

    // Keep the history within the spread of this frame's neighbours (variance clipping).
    mean /= 9.0;
    let sigma = sqrt(max(square / 9.0 - mean * mean, vec3<f32>(0.0)));
    let box_lo = max(lo, mean - sigma * 1.25);
    let box_hi = min(hi, mean + sigma * 1.25);
    let old = clip_to_box(to_ycocg(squash(sample_history(history_uv))), box_lo, box_hi);

    // Take more of this frame when the camera moves fast, where the history is least reliable.
    let speed = length(motion / taa.params.zw);
    let weight = mix(taa.params.y, 0.35, saturate(speed / 30.0));
    let blended = mix(old, center, weight);
    return vec4<f32>(unsquash(from_ycocg(blended)), 1.0);
}
