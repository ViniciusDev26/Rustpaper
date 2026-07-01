// Declara os módulos do projeto. Cada `mod X;` faz o Rust carregar `src/X.rs`.
mod gpu;
mod video;
mod wallpaper;

fn main() {
    // Sobe o wallpaper: cria a layer surface no fundo do desktop e renderiza.
    wallpaper::run();
}
