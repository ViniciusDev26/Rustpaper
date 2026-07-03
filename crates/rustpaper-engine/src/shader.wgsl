// Shader de wallpaper: triângulo de tela cheia amostrando uma textura.
// - scale:   cover (preenche a tela mantendo a proporção do conteúdo)
// - content: fração da textura que é conteúdo real (o resto é padding, ignorado)

struct Uniforms {
    scale: vec2<f32>,
    content: vec2<f32>,
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
    // origem no topo-esquerdo (imagem/vídeo têm a linha 0 no topo)
    var s = vec2<f32>(in.uv.x, 1.0 - in.uv.y);
    // cover: escala em torno do centro
    s = (s - 0.5) * u.scale + 0.5;
    // recorta pra região de conteúdo (ignora o padding do buffer da textura)
    s = s * u.content;
    // alpha forçado a 1: wallpaper é fundo OPACO. Sem isso, texturas com alpha < 1
    // deixam a surface translúcida e o compositor mistura com o fundo -> flicker.
    return vec4<f32>(textureSample(tex, samp, s).rgb, 1.0);
}
