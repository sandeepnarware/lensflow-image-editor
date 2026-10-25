// The web's VERTEX: one triangle [-1,-1, 3,-1, -1,3] covering the target, vUv = aPos * 0.5 + 0.5.
//
// Orientation: in WebGL, vUv.y = 0 is texture row 0 and framebuffer row 0, so every
// intermediate target keeps row 0 = the image's top. In wgpu, framebuffer row 0 is NDC
// y = +1, so the triangle is mirrored in y on output; uv keeps the web's formula and the
// same meaning (uv.y = 0 is row 0 of both the sampled texture and the target).

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    var corners = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let pos = corners[index];
    var out: VsOut;
    out.uv = pos * 0.5 + 0.5;
    out.position = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
    return out;
}
