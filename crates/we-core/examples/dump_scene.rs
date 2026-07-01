// `cargo run -p we-core --example dump_scene -- <scene.pkg>`
// Abre o pkg, acha a textura de fundo (scene.json -> model -> material) e a
// decodifica (.tex -> RGBA), imprimindo as dimensões.

use std::path::Path;
use we_core::{pkg::Pkg, scene, tex};

fn main() {
    let path = std::env::args().nth(1).expect("uso: dump_scene <scene.pkg>");
    let pkg = Pkg::open(Path::new(&path)).expect("falha ao abrir pkg");

    let tex_path = scene::background_texture(&pkg).expect("não achei textura de fundo");
    println!("textura de fundo: {tex_path}");

    let bytes = pkg.read(&tex_path).expect("textura não está no pkg");
    println!(".tex: {} bytes", bytes.len());

    match tex::parse(bytes) {
        Ok(t) => {
            println!(
                "decodificado: buffer {}x{}, conteúdo {}x{}, rgba {} bytes",
                t.width, t.height, t.real_width, t.real_height, t.rgba.len()
            );
            assert_eq!(t.rgba.len(), (t.width * t.height * 4) as usize);
            println!("OK ✓ (rgba bate com width*height*4)");
        }
        Err(e) => println!("ERRO: {e}"),
    }
}
