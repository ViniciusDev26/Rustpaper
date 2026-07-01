// Shader de wallpaper: triângulo de tela cheia amostrando uma textura.
// O "cover" agora é calculado na CPU (testável) e chega pronto em `scale`.

struct Uniforms {
    scale: vec2<f32>, // fator de cover (aplicado ao uv em torno do centro)
    time: f32,
};
@group(0) @binding(0) var<uniform> u: Uniforms;
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
    // Y invertido (imagem/vídeo têm origem no topo) + cover aplicado.
    var uv = vec2<f32>(in.uv.x, 1.0 - in.uv.y);
    uv = (uv - 0.5) * u.scale + 0.5;

    return textureSample(tex, samp, uv);
}
