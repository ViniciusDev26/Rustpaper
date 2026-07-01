mod gpu;
mod pkg;
mod project;
mod scene;
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
