// Declara os módulos do projeto. Cada `mod X;` faz o Rust carregar `src/X.rs`.
mod gpu;
// Ainda não conectado à app (será o carregador que escolhe o arquivo por tipo);
// por ora existe com seus testes. allow(dead_code) evita warnings até ligarmos.
#[allow(dead_code)]
mod project;
mod video;
mod wallpaper;

fn main() {
    // Sobe o wallpaper: cria a layer surface no fundo do desktop e renderiza.
    wallpaper::run();
}
