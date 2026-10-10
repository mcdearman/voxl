// Lines drawn over the finished frame, each a quad as wide as asked for in pixels, with
// soft edges, and faint where the scene is in front of it. One instance is one line; the six corners come from the vertex index.

struct View {
    view_proj: mat4x4<f32>,
    // xy: the frame's size in pixels, z: how much of a hidden line shows.
    size: vec4<f32>,
}

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var depth: texture_depth_multisampled_2d;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    // x: pixels from the line's middle, y: half its width.
    @location(1) across: vec2<f32>,
    // The depth the line itself is at, as clip z and w.
    @location(2) deep: vec2<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) index: u32,
    @location(0) start: vec4<f32>,
    @location(1) end: vec4<f32>,
    @location(2) color: vec4<f32>,
) -> Out {
    var ends = array<f32, 6>(0.0, 1.0, 0.0, 0.0, 1.0, 1.0);
    var sides = array<f32, 6>(-1.0, -1.0, 1.0, 1.0, -1.0, 1.0);
    var out: Out;
    out.color = color;
    var p = view.view_proj * vec4<f32>(start.xyz, 1.0);
    var q = view.view_proj * vec4<f32>(end.xyz, 1.0);
    // Cut the line where it passes behind the eye.
    let near = 0.001;
    if p.w < near && q.w < near {
        out.position = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    if p.w < near {
        p = mix(p, q, (near - p.w) / (q.w - p.w));
    } else if q.w < near {
        q = mix(q, p, (near - q.w) / (p.w - q.w));
    }
    let half_frame = view.size.xy * 0.5;
    let along = q.xy / q.w * half_frame - p.xy / p.w * half_frame;
    let long = length(along);
    var direction = vec2<f32>(1.0, 0.0);
    if long > 0.00001 {
        direction = along / long;
    }
    let half_width = start.w * 0.5;
    // Half a pixel more than the line, for the soft edge.
    let reach = sides[index] * (half_width + 0.5);
    let corner = mix(p, q, ends[index]);
    let offset = vec2<f32>(-direction.y, direction.x) * reach / half_frame;
    // The pass has no depth buffer, so any depth inside the frame will do here; the line's
    // own goes to the fragment shader, to set against the scene's.
    out.position = vec4<f32>(corner.xy + offset * corner.w, 0.5 * corner.w, corner.w);
    out.across = vec2<f32>(reach, half_width);
    out.deep = corner.zw;
    return out;
}

@fragment
fn fs_main(in: Out) -> @location(0) vec4<f32> {
    let cover = clamp(in.across.y + 0.5 - abs(in.across.x), 0.0, 1.0);
    // Reversed depth: nearer is greater. A line a little behind a surface still counts as on
    // it, so shapes drawn where meshes are don't flicker.
    let scene = textureLoad(depth, vec2<i32>(in.position.xy), 0);
    let mine = in.deep.x / in.deep.y;
    var seen = 1.0;
    if mine < scene * 0.98 - 0.0005 {
        seen = view.size.z;
    }
    return vec4<f32>(in.color.rgb, in.color.a * cover * seen);
}
