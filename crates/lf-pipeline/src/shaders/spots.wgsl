// The web's SPOTS (shaders.ts), line for line: heal and clone spots, drawn at the photo's own
// size from the source into a mipmapped target. (`from` is reserved in WGSL: `from_uv`.)
//
// Heal: the copied patch is shifted in tone to match the spot's edge. The difference is
// measured on a ring just outside both circles (so the blemish itself never counts) and
// blended across the spot by angle. Samples inside the spot loop use
// textureSampleLevel(…, 0.0), as the GLSL's textureLod(…, 0.0): the loop branches per pixel,
// where implicit derivatives (and so the mip level) are undefined.
//
// One deliberate difference, as in develop.wgsl: the main sample's `texture()` (implicit
// derivatives) becomes `textureSampleGrad` with the derivatives a quad gives at 1:1 (one texel
// per pixel, so level 0), independent of helper lanes past the target's edge.

const MAX_SPOTS: u32 = 48u;

struct Uniforms {
    spot: array<vec4<f32>, 48>,  // uSpot: target x, y, source x, y (UV)
    spot_b: array<vec4<f32>, 48>, // uSpotB (xy): radius (fraction of width), mode (0 heal, 1 clone)
    size: vec2<f32>,             // uSize: photo size in pixels
    count: u32,                  // uCount
    _pad: u32,
}

@group(0) @binding(0) var u_tex: texture_2d<f32>;
@group(0) @binding(1) var s_tex: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;

fn ring_diff(target_uv: vec2<f32>, source_uv: vec2<f32>, radius: vec2<f32>, angle: f32) -> vec3<f32> {
    let dir = vec2<f32>(cos(angle), sin(angle)) * radius * 1.15;
    return textureSampleLevel(u_tex, s_tex, target_uv + dir, 0.0).rgb
        - textureSampleLevel(u_tex, s_tex, source_uv + dir, 0.0).rgb;
}

@fragment
fn fs_spots(in: VsOut) -> @location(0) vec4<f32> {
    let v_uv = in.uv;
    var color = textureSampleGrad(u_tex, s_tex, v_uv, vec2<f32>(1.0 / u.size.x, 0.0), vec2<f32>(0.0, 1.0 / u.size.y)).rgb;
    for (var i = 0u; i < MAX_SPOTS; i++) {
        if (i >= u.count) {
            break;
        }
        let s = u.spot[i];
        let r = u.spot_b[i].x * u.size.x;
        let offset = (v_uv - s.xy) * u.size;
        let d = length(offset) / r;
        if (d >= 1.0) {
            continue;
        }
        let w = 1.0 - smoothstep(0.6, 1.0, d);
        let from_uv = v_uv + (s.zw - s.xy);
        var fix = textureSampleLevel(u_tex, s_tex, from_uv, 0.0).rgb;
        if (u.spot_b[i].y < 0.5) {
            let radius = vec2<f32>(r) / u.size;
            var sum = vec3<f32>(0.0);
            var total = 0.0;
            for (var k = 0; k < 12; k++) {
                let a = f32(k) * 0.5235988;
                let ring = vec2<f32>(cos(a), sin(a)) * r * 1.15;
                let wk = 1.0 / (dot(offset - ring, offset - ring) / (r * r) + 0.05);
                sum += ring_diff(s.xy, s.zw, radius, a) * wk;
                total += wk;
            }
            fix += sum / total;
        }
        color = mix(color, fix, w);
    }
    return vec4<f32>(color, 1.0);
}
