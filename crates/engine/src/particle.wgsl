// Shader de partículas: desenha um quad por instância (billboard) amostrando o
// sprite. A geometria do quad é gerada por vertex_index; posição/tamanho/cor vêm
// do buffer de instância.

struct Instance {
    @location(0) center: vec2<f32>, // clip space
    @location(1) half: vec2<f32>,   // meio-tamanho em clip
    @location(2) color: vec4<f32>,
};

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32, inst: Instance) -> VOut {
    // 6 vértices = 2 triângulos formando um quad de -1..1.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0),  vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[vi];
    var out: VOut;
    out.pos = vec4<f32>(inst.center + c * inst.half, 0.0, 1.0);
    out.uv = c * 0.5 + 0.5;
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
    // sprite * cor (tint + alpha do fade). O alpha do sprite (halo radial) dá o
    // formato macio da partícula.
    return textureSample(tex, samp, in.uv) * in.color;
}
