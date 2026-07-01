// Shader de WALLPAPER: um triângulo de tela cheia + efeito procedural no
// fragment shader (roda por pixel). É assim que wallpapers animados funcionam.

struct Uniforms {
    time: f32,
    resolution: vec2<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Agora passamos UV (coordenada 0..1 na tela) em vez de cor.
    @location(0) uv: vec2<f32>,
};

// === VERTEX SHADER ===
// Três vértices que formam um triângulo GIGANTE cobrindo toda a tela [-1,1].
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),  // canto inferior esquerdo
        vec2<f32>( 3.0, -1.0),  // muito à direita (fora da tela)
        vec2<f32>(-1.0,  3.0),  // muito acima (fora da tela)
    );
    let p = positions[index];

    var out: VertexOutput;
    out.clip_position = vec4<f32>(p, 0.0, 1.0);
    // Converte clip space (-1..1) pra UV (0..1). A parte que passa de 1 fica
    // fora da tela e nunca vira pixel — só a região visível 0..1 é desenhada.
    out.uv = p * 0.5 + 0.5;
    return out;
}

// === FRAGMENT SHADER === (roda por pixel — aqui mora o "efeito")
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = u.time;
    let uv = in.uv;

    // Plasma: soma de senos em X, Y e diagonal, defasados no tempo. Cada canal de
    // cor usa uma combinação diferente -> cores fluindo pela tela.
    let r = 0.5 + 0.5 * sin(t + uv.x * 6.28);
    let g = 0.5 + 0.5 * sin(t * 1.3 + uv.y * 6.28 + 2.0);
    let b = 0.5 + 0.5 * sin(t * 0.7 + (uv.x + uv.y) * 6.28 + 4.0);

    return vec4<f32>(r, g, b, 1.0);
}
