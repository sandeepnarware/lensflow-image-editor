
// The web's DEVELOP (shaders.ts), line for line: every adjustment, geometry and effects,
// straight to the output.
//
// Deliberate differences from the GLSL (see also develop() below): `texture()` (implicit derivatives) becomes
// `textureSampleGrad` with the derivatives of `uv` worked out from the uniforms (`pixel`, the
// size of one output pixel in canvas UV, is the only uniform DEVELOP does not have). The
// value is the one a quad's differences give for this affine `uv`, so the mip level is the
// GLSL's; but it does not depend on helper lanes past the target's last row or column, which
// some drivers (WARP) fill with garbage, and it allows the early return for pixels outside
// the frame. As the brief asks, the 5×5 noise loop and the curve lookups sample level 0.

struct Uniforms {
    matrix: mat3x3<f32>,    // uMatrix: output UV → photo UV
    view: vec4<f32>,        // uView: canvas UV → output UV (zoom/pan): x, y, w, h
    img_size: vec2<f32>,    // uImgSize: working photo size (pixels)
    out_size: vec2<f32>,    // uOutSize: output size at full resolution (pixels)
    scale: f32,             // uScale: working size ÷ full size
    flip_y: u32,            // uFlipY: drawing to the canvas
    develop: u32,           // uDevelop: 0 = the photo as is (before/after)
    exposure: f32,
    contrast: f32,
    highlights: f32,
    shadows: f32,
    whites: f32,
    blacks: f32,
    vibrance: f32,
    saturation: f32,
    grade_balance: f32,
    wb: vec3<f32>,
    clarity: f32,
    grade_sh: vec3<f32>,
    texture_amount: f32,
    grade_mid: vec3<f32>,
    dehaze: f32,
    grade_hi: vec3<f32>,
    sharpen: f32,
    noise: f32,
    color_noise: f32,
    vignette: f32,
    vignette_mid: f32,
    grain: f32,
    grain_size: f32,
    bw: u32,
    sharp_on: u32,
    tex_on: u32,
    _pad0: u32,
    pixel: vec2<f32>,       // one output pixel in canvas UV (1 / target size); WGSL only
    hsl: array<vec4<f32>, 8>,        // uHsl (xyz)
    band_hue: array<vec4<f32>, 8>,   // uBandHue (x)
    band_width: array<vec4<f32>, 8>, // uBandWidth (x)
}

@group(0) @binding(0) var u_img: texture_2d<f32>;    // the photo (after spots)
@group(0) @binding(1) var u_blur_m: texture_2d<f32>; // wide blurs (low resolution)
@group(0) @binding(2) var u_blur_l: texture_2d<f32>;
@group(0) @binding(3) var u_blur_s: texture_2d<f32>; // sharpening blur
@group(0) @binding(4) var u_blur_t: texture_2d<f32>; // texture blur
@group(0) @binding(5) var u_curve: texture_2d<f32>;  // 256×1, per-channel curve (master already applied)
@group(0) @binding(6) var s_img: sampler;
@group(0) @binding(7) var s_blur_m: sampler;
@group(0) @binding(8) var s_blur_l: sampler;
@group(0) @binding(9) var s_blur_s: sampler;
@group(0) @binding(10) var s_blur_t: sampler;
@group(0) @binding(11) var s_curve: sampler;
@group(0) @binding(12) var<uniform> u: Uniforms;

// d(uv)/dx and d(uv)/dy per output pixel, set by fs_develop.
var<private> grad_x: vec2<f32>;
var<private> grad_y: vec2<f32>;

// The GLSL's texture(t, uv): implicit derivatives of the frame's affine uv.
fn sample_2d(t: texture_2d<f32>, s: sampler, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleGrad(t, s, uv, grad_x, grad_y);
}

const LUMA = vec3<f32>(0.2126, 0.7152, 0.0722);
fn luma(c: vec3<f32>) -> f32 { return dot(c, LUMA); }
fn to_linear(c: vec3<f32>) -> vec3<f32> { return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2)); }
fn to_gamma(c: vec3<f32>) -> vec3<f32> { return pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)); }

// GLSL's mod: x - y * floor(x / y).
fn glsl_mod(x: f32, y: f32) -> f32 { return x - y * floor(x / y); }

fn rgb2hsv(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4<f32>(c.bg, K.wz), vec4<f32>(c.gb, K.xy), step(c.b, c.g));
    let q = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}
fn hsv2rgb(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

fn hash(p_in: vec2<f32>) -> f32 {
    var p = fract(p_in * vec2<f32>(443.897, 441.423));
    p += dot(p, p.yx + 19.19);
    return fract((p.x + p.y) * p.x);
}
fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    var f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), f.x), mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), f.x), f.y);
}

// One curve lookup at the texel centres: x = 0 → texel 0, x = 1 → texel 255.
fn curve_at(x: f32) -> vec4<f32> {
    return textureSampleLevel(u_curve, s_curve, vec2<f32>(x * 255.0 / 256.0 + 0.5 / 256.0, 0.5), 0.0);
}

// The GLSL's develop() after its first two lines (`c = texture(uImg, uv)`, and the early
// return when uDevelop is off), which fs_develop does: with FXC's optimiser that early return
// made WARP lose the device.
fn develop(photo: vec3<f32>, uv: vec2<f32>) -> vec3<f32> {
    var c = photo;

    // Noise reduction: an edge-aware average for brightness, a plain one for colour.
    if (u.noise > 0.0 || u.color_noise > 0.0) {
        let px = max(1.6 * u.scale, 0.75) / u.img_size;
        let yc = luma(c);
        let sigma_r = 0.02 + u.noise * 0.12;
        var sum_f = vec3<f32>(0.0);
        var sum_c = vec3<f32>(0.0);
        var w_f = 0.0;
        var w_c = 0.0;
        for (var y = -2; y <= 2; y++) {
            for (var x = -2; x <= 2; x++) {
                let s = textureSampleLevel(u_img, s_img, uv + vec2<f32>(f32(x), f32(y)) * px, 0.0).rgb;
                let ws = exp(-f32(x * x + y * y) / 4.5);
                let dy = luma(s) - yc;
                let w = ws * exp(-dy * dy / (2.0 * sigma_r * sigma_r));
                sum_f += s * w;
                w_f += w;
                sum_c += s * ws;
                w_c += ws;
            }
        }
        let f = sum_f / w_f;
        let a = sum_c / w_c;
        let y = mix(yc, luma(f), u.noise);
        let chroma = mix(c - yc, a - luma(a), u.color_noise);
        c = y + chroma;
    }

    // Texture (fine detail) and sharpening (edges, brightness only so colours don't fringe).
    if (u.tex_on != 0u) { c += (c - sample_2d(u_blur_t, s_blur_t, uv).rgb) * u.texture_amount * 1.4; }
    if (u.sharp_on != 0u) {
        let d = luma(c - sample_2d(u_blur_s, s_blur_s, uv).rgb);
        let mask = smoothstep(0.004, 0.02, abs(d));
        c += d * u.sharpen * 1.8 * mask;
    }

    var bm = sample_2d(u_blur_m, s_blur_m, uv).rgb;
    let bl = sample_2d(u_blur_l, s_blur_l, uv).rgb;

    // Dehaze: lifts the haze estimated from the darkest channel of the wide blur.
    if (u.dehaze > 0.0) {
        let t = max(1.0 - u.dehaze * 0.6 * min(min(bl.r, bl.g), bl.b), 0.2);
        c = (c - (1.0 - t)) / t;
        bm = (bm - (1.0 - t)) / t;
    } else if (u.dehaze < 0.0) {
        let k = -u.dehaze * 0.55;
        c = mix(c, vec3<f32>(0.78 + 0.22 * luma(bl)), k * (0.35 + 0.65 * luma(bl)));
        bm = mix(bm, vec3<f32>(0.78 + 0.22 * luma(bl)), k * (0.35 + 0.65 * luma(bl)));
    }

    // White balance and exposure, in linear light.
    let gain = exp2(u.exposure);
    c = to_gamma(to_linear(c) * u.wb * gain);
    let yb = luma(to_gamma(to_linear(bm) * u.wb * gain));

    // Highlights and shadows follow the local brightness (the wide blur), so whole areas move together.
    let y = max(luma(c), 1.0e-4);
    let local = clamp(mix(y, yb, 0.65), 0.0, 1.0);
    let w_s = pow(1.0 - local, 2.0);
    let w_h = pow(local, 2.0);
    var yn = y;
    if (u.highlights < 0.0) { yn *= 1.0 + u.highlights * 0.3 * w_h; }
    else { yn += u.highlights * w_h * 0.6 * yn * max(1.0 - yn, 0.0); }
    if (u.shadows > 0.0) { yn += u.shadows * w_s * 0.5 * sqrt(yn) * max(1.0 - yn, 0.0); }
    else { yn += u.shadows * w_s * 0.75 * yn * max(1.0 - yn, 0.0); }
    c *= yn / y;

    // Whites and blacks move the end points.
    let bp = -u.blacks * 0.06;
    let wp = 1.0 - u.whites * 0.2;
    c = (c - bp) / (wp - bp);

    // Clarity: midtone local contrast against the wide blur.
    if (u.clarity != 0.0) {
        let y2 = max(luma(c), 1.0e-4);
        let ybl = (yb - bp) / (wp - bp);
        let mid = 1.0 - pow(clamp(2.0 * y2 - 1.0, -1.0, 1.0), 2.0);
        let y3 = max(y2 + u.clarity * 0.8 * (y2 - ybl) * mid, 0.0);
        c *= y3 / y2;
    }

    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));

    // Contrast: an S-curve that never clips.
    c = c - u.contrast * 0.8 * sin(6.2831853 * c) / 6.2831853;

    // Tone curve.
    c = vec3<f32>(curve_at(c.r).r, curve_at(c.g).g, curve_at(c.b).b);

    // Colour mix: each band's hue, saturation and luminance.
    var hsv = rgb2hsv(c);
    let hue = hsv.x * 360.0;
    var adj = vec3<f32>(0.0);
    for (var i = 0; i < 8; i++) {
        let d = abs(glsl_mod(hue - u.band_hue[i].x + 540.0, 360.0) - 180.0);
        let w = clamp(1.0 - d / u.band_width[i].x, 0.0, 1.0);
        adj += w * w * (3.0 - 2.0 * w) * u.hsl[i].xyz;
    }
    let sat_weight = smoothstep(0.0, 0.25, hsv.y);

    if (u.bw != 0u) {
        // Black & white: the colour-mix luminance sliders choose how bright each colour turns.
        let g = luma(c) * (1.0 + adj.z * 0.7 * sat_weight);
        c = vec3<f32>(clamp(g, 0.0, 1.0));
    } else {
        hsv.x = fract(hsv.x + adj.x * sat_weight * 30.0 / 360.0 + 1.0);
        hsv.y = clamp(hsv.y * (1.0 + adj.y), 0.0, 1.0);
        c = hsv2rgb(hsv);
        c *= 1.0 + adj.z * 0.45 * sat_weight;

        let s = rgb2hsv(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0))).y;
        let yv = luma(c);
        // Vibrance boosts dull colours more than strong ones, and skin tones least.
        let skin = 1.0 - smoothstep(10.0, 40.0, abs(hue - 25.0));
        let vib = u.vibrance * (1.0 - s) * select(1.0, 1.0 - 0.6 * skin, u.vibrance > 0.0);
        c = mix(vec3<f32>(yv), c, 1.0 + vib);
        c = mix(vec3<f32>(yv), c, 1.0 + u.saturation);
    }

    // Colour grading: tints for shadows, midtones and highlights.
    let yg = clamp(luma(c), 0.0, 1.0);
    let pivot = 0.5 + u.grade_balance * 0.25;
    let gs = 1.0 - smoothstep(0.0, pivot, yg);
    let gh = smoothstep(pivot, 1.0, yg);
    let gm = clamp(1.0 - gs - gh, 0.0, 1.0);
    c += u.grade_sh * gs + u.grade_mid * gm + u.grade_hi * gh;

    return c;
}

@fragment
fn fs_develop(in: VsOut) -> @location(0) vec4<f32> {
    let canvas_uv = vec2<f32>(in.uv.x, select(in.uv.y, 1.0 - in.uv.y, u.flip_y != 0u));
    let out_uv = u.view.xy + canvas_uv * u.view.zw;
    let uv = (u.matrix * vec3<f32>(out_uv, 1.0)).xy;
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0 || out_uv.x < 0.0 || out_uv.y < 0.0 || out_uv.x > 1.0 || out_uv.y > 1.0) {
        return vec4<f32>(0.12, 0.12, 0.13, 1.0);
    }
    grad_x = (u.matrix * vec3<f32>(u.view.z * u.pixel.x, 0.0, 0.0)).xy;
    grad_y = (u.matrix * vec3<f32>(0.0, u.view.w * u.pixel.y * select(1.0, -1.0, u.flip_y != 0u), 0.0)).xy;

    var c = sample_2d(u_img, s_img, uv).rgb;
    if (u.develop != 0u) { c = develop(c, uv); }

    if (u.develop != 0u && u.vignette != 0.0) {
        // Elliptical, following the crop's shape; the midpoint sets where it starts.
        let d = length((out_uv - 0.5) * 2.0) / 1.41421356;
        let start = mix(0.05, 0.85, u.vignette_mid);
        let f = smoothstep(start, 1.15, d);
        c = select(mix(c, vec3<f32>(1.0), u.vignette * f), c * (1.0 + u.vignette * f), u.vignette < 0.0);
    }

    if (u.develop != 0u && u.grain > 0.0) {
        // Grain sized in output pixels, so the preview and the export match.
        let cell = 0.7 + u.grain_size * 3.0;
        let p = out_uv * u.out_size / cell;
        let n = value_noise(p) * 0.6 + value_noise(p * 2.13 + 17.0) * 0.4 - 0.5;
        let y = luma(c);
        c += n * u.grain * 0.22 * (0.4 + 0.6 * (1.0 - abs(2.0 * y - 1.0)));
    }

    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
