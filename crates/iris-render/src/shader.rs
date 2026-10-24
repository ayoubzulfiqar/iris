//! Full-screen triangle. UV is computed from the unscaled corner, then the
//! model-view-projection matrix moves the clip position, so Fit and Center
//! scale the quad without changing which texel lands on a corner.

pub const SHADER: &str = r#"
struct Uniforms {
    time: f32,
    delta_time: f32,
    mouse: vec2<f32>,
    resolution: vec2<f32>,
    padding: vec2<f32>,
    g_ModelViewProjectionMatrix: mat4x4<f32>,
    uv_scale: vec2<f32>,
    uv_offset: vec2<f32>,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(1) @binding(0) var tex_y: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
@group(1) @binding(2) var tex_u: texture_2d<f32>;
@group(1) @binding(3) var tex_v: texture_2d<f32>;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) live: f32,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var pos = vec2<f32>(-1.0, -1.0);
    if (index == 1u) {
        pos = vec2<f32>(3.0, -1.0);
    } else if (index == 2u) {
        pos = vec2<f32>(-1.0, 3.0);
    }
    var out: VsOut;
    out.clip = u.g_ModelViewProjectionMatrix * vec4<f32>(pos, 0.0, 1.0);
    let uv = vec2<f32>(pos.x * 0.5 + 0.5, 0.5 - pos.y * 0.5);
    out.uv = uv * u.uv_scale + u.uv_offset;
    let g_Time = u.time;
    let g_PointerPosition = u.mouse;
    out.live = g_Time * 0.0
        + u.delta_time * 0.0
        + g_PointerPosition.x * 0.0
        + g_PointerPosition.y * 0.0
        + u.resolution.x * 0.0
        + u.resolution.y * 0.0
        + u.padding.x * 0.0
        + u.padding.y * 0.0;
    return out;
}

// Limited-range studio swing. Coefficients match `yuv::limited_rgb`.
fn rec709(y: f32, u: f32, v: f32) -> vec3<f32> {
    let yy = 1.164383 * (y - 16.0 / 255.0);
    let uu = u - 128.0 / 255.0;
    let vv = v - 128.0 / 255.0;
    return clamp(
        vec3<f32>(
            yy + 1.792741 * vv,
            yy - 0.213249 * uu - 0.532909 * vv,
            yy + 2.112402 * uu,
        ),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

fn rec601(y: f32, u: f32, v: f32) -> vec3<f32> {
    let yy = 1.164383 * (y - 16.0 / 255.0);
    let uu = u - 128.0 / 255.0;
    let vv = v - 128.0 / 255.0;
    return clamp(
        vec3<f32>(
            yy + 1.596027 * vv,
            yy - 0.391762 * uu - 0.812968 * vv,
            yy + 2.017232 * uu,
        ),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let mode = u32(u.padding.x + 0.5);
    if (mode == 0u) {
        let sampled = textureSample(tex_y, samp, in.uv);
        return vec4<f32>(sampled.rgb, sampled.a + in.live * 0.0);
    }
    let y = textureSample(tex_y, samp, in.uv).r;
    var chroma_u: f32;
    var chroma_v: f32;
    if (mode == 2u || mode == 4u) {
        let uv = textureSample(tex_u, samp, in.uv);
        chroma_u = uv.r;
        chroma_v = uv.g;
    } else {
        chroma_u = textureSample(tex_u, samp, in.uv).r;
        chroma_v = textureSample(tex_v, samp, in.uv).r;
    }
    let bt709 = mode == 1u || mode == 2u;
    let rgb = select(rec601(y, chroma_u, chroma_v), rec709(y, chroma_u, chroma_v), bt709);
    return vec4<f32>(rgb, 1.0 + in.live * 0.0);
}
"#;
