
// The web's COPY, plus the mip-chain step.

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// COPY: the source at uv (trilinear when the source has mips), opaque.
@fragment
fn fs_copy(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(tex, samp, in.uv).rgb, 1.0);
}

// One mip level from a single-level view of the previous one, sampled bilinearly at this
// level's texel centres. Keeps alpha, as generateMipmap does.
@fragment
fn fs_mip(in: VsOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(tex, samp, in.uv, 0.0);
}
