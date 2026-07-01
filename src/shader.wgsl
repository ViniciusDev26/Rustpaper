// Shader de wallpaper: triângulo de tela cheia amostrando uma TEXTURA (imagem).

struct Uniforms {
    time: f32,
    resolution: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
// A textura (imagem) e o sampler (regras de leitura). Bindings 1 e 2.
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 3.0, -1.0),
        vec2<f32>(-1.0,  3.0),
    );
    let p = positions[index];
    var out: VertexOutput;
    out.clip_position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + 0.5;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Inverte o Y: imagens têm a linha 0 no TOPO; nosso uv tem origem embaixo.
    let uv = vec2<f32>(in.uv.x, 1.0 - in.uv.y);
    // textureSample lê a cor da textura no ponto uv, aplicando o sampler.
    var color = textureSample(tex, samp, uv);
    // Pulsação sutil de brilho pelo tempo (prova que o uniform ainda funciona).
    color = color * (0.85 + 0.15 * sin(u.time));
    return color;
}
