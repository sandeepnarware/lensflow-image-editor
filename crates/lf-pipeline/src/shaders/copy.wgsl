
// The web's COPY, plus the mip-chain step.

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct CopyParams {
    pixel: vec2<f32>, // one destination pixel in UV (1 / destination size)
    _pad: vec2<f32>,
}
@group(0) @binding(2) var<uniform> params: CopyParams; // fs_copy only

// COPY: the source at uv (trilinear when the source has mips), opaque. The GLSL's
// `texture()` takes implicit derivatives; for this fullscreen uv they are one destination
// pixel, d(uv)/dx = (1 / dst_w, 0) and d(uv)/dy = (0, 1 / dst_h) (the source/destination
// ratio over the source size), passed explicitly so the mip level does not depend on helper
// lanes past the target's edge (garbage on WARP for odd-sized small copies).
@fragment
fn fs_copy(in: VsOut) -> @location(0) vec4<f32> {
    let dx = vec2<f32>(params.pixel.x, 0.0);
    let dy = vec2<f32>(0.0, params.pixel.y);
    return vec4<f32>(textureSampleGrad(tex, samp, in.uv, dx, dy).rgb, 1.0);
}

// One mip level from a single-level view of the previous one, sampled bilinearly at this
// level's texel centres. Keeps alpha, as generateMipmap does.
@fragment
fn fs_mip(in: VsOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(tex, samp, in.uv, 0.0);
}
