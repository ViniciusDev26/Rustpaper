// Biblioteca do engine: expõe os módulos pra serem reutilizados pelo binário
// (main.rs) e pelos examples/testes (ex.: render offscreen de cenas).
pub mod compositor;
pub mod gpu;
pub mod particles;
pub mod postprocess;
pub mod program;
pub mod shader_compile;
pub mod video;
pub mod wallpaper;
