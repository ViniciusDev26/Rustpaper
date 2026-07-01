mod gpu;
// Leitor de .pkg (fase 1 das cenas). Ainda não conectado à app; existe com testes
// e o example dump_pkg. allow(dead_code) evita warnings até ligarmos.
#[allow(dead_code)]
mod pkg;
mod project;
// Fases 2 e 3 das cenas (parser do scene.json e decodificador .tex). Ainda não
// ligados ao render (fase 4); existem com testes + os examples dump_*.
#[allow(dead_code)]
mod scene;
#[allow(dead_code)]
mod tex;
mod video;
mod wallpaper;

use std::path::PathBuf;

fn main() {
    // Recebe a pasta do wallpaper (a de um item do Workshop, com project.json).
    // Sem argumento: mostra o uso e sai.
    let dir = match std::env::args().nth(1) {
        Some(d) => PathBuf::from(d),
        None => {
            eprintln!("uso: wallpaper-engine-rs <pasta-do-wallpaper>");
            eprintln!("ex.: wallpaper-engine-rs /home/vscode/wallpapers/2499404313");
            std::process::exit(2);
        }
    };
    wallpaper::run(&dir);
}
