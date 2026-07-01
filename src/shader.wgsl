// Shader de wallpaper: triângulo de tela cheia amostrando uma textura, com
// "cover" (preenche a tela mantendo a proporção da imagem, cortando o excesso).

struct Uniforms {
    resolution: vec2<f32>, // tamanho da tela (px)
    image_size: vec2<f32>, // tamanho da imagem (px)
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
    // Y invertido (imagem tem origem no topo).
    var uv = vec2<f32>(in.uv.x, 1.0 - in.uv.y);

    // COVER: escala a amostragem em torno do centro (0.5). Encolhe o eixo que
    // "sobra" pra mostrar só a fatia central com a proporção da tela.
    let screen_aspect = u.resolution.x / u.resolution.y;
    let image_aspect = u.image_size.x / u.image_size.y;
    var scale = vec2<f32>(1.0, 1.0);
    if (screen_aspect > image_aspect) {
        // Tela mais larga que a imagem: preenche a largura, corta em cima/baixo.
        scale.y = image_aspect / screen_aspect;
    } else {
        // Tela mais "alta": preenche a altura, corta nas laterais.
        scale.x = screen_aspect / image_aspect;
    }
    uv = (uv - 0.5) * scale + 0.5;

    var color = textureSample(tex, samp, uv);
    color = color * (0.85 + 0.15 * sin(u.time)); // pulsação sutil
    return color;
}
