// Shader em WGSL (roda na GPU). Agora com COR POR VÉRTICE + interpolação.

// Uma struct pra descrever o que o vertex shader ENTREGA ao fragment shader.
// - @builtin(position): a posição (obrigatória; a GPU usa pra rasterizar).
// - @location(0): um dado NOSSO (a cor). O número 0 é o "canal" que liga o
//   vertex ao fragment: o vertex escreve em @location(0), o fragment lê de lá.
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) color: vec3<f32>,
};

// === VERTEX SHADER === (roda 1x por vértice)
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    // Posições dos 3 cantos (x, y) em clip space (-1..+1).
    var positions = array<vec2<f32>, 3>(
        vec2<f32>( 0.0,  0.5),  // topo
        vec2<f32>(-0.5, -0.5),  // inferior esquerdo
        vec2<f32>( 0.5, -0.5),  // inferior direito
    );
    // Uma cor (RGB) para cada canto.
    var colors = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.0, 0.0),  // vértice 0: vermelho
        vec3<f32>(0.0, 1.0, 0.0),  // vértice 1: verde
        vec3<f32>(0.0, 0.0, 1.0),  // vértice 2: azul
    );

    var out: VertexOutput;
    out.clip_position = vec4<f32>(positions[index], 0.0, 1.0);
    out.color = colors[index];
    return out;
}

// === FRAGMENT SHADER === (roda 1x por pixel)
// `in` chega com a cor JÁ INTERPOLADA para este pixel específico — a GPU mistura
// as cores dos 3 cantos conforme a distância. Só precisamos devolvê-la.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
