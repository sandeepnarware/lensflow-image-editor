
// The web's GAUSSIAN: one separable run along `step`.

struct Params {
    step: vec2<f32>, // one texel along the blur direction
    sigma: f32,      // in texels
    _pad: f32,
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> params: Params;

@fragment
fn fs_gaussian(in: VsOut) -> @location(0) vec4<f32> {
    let sigma = params.sigma;
    let radius = i32(min(ceil(sigma * 3.0), 64.0));
    var total = 1.0;
    var sum = textureSampleLevel(tex, samp, in.uv, 0.0).rgb;
    for (var i = 1; i <= 64; i++) {
        if (i > radius) {
            break;
        }
        let w = exp(-f32(i * i) / (2.0 * sigma * sigma));
        let offset = params.step * f32(i);
        sum += (textureSampleLevel(tex, samp, in.uv + offset, 0.0).rgb
            + textureSampleLevel(tex, samp, in.uv - offset, 0.0).rgb) * w;
        total += 2.0 * w;
    }
    return vec4<f32>(sum / total, 1.0);
}
