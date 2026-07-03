// `cargo run -p rustpaper-core --example dump_pkg -- <arquivo.pkg>`
// Lista os arquivos de um .pkg do Wallpaper Engine.

use std::path::Path;
use rustpaper_core::pkg::Pkg;

fn main() {
    let path = std::env::args().nth(1).expect("uso: dump_pkg <arquivo.pkg>");
    let pkg = Pkg::open(Path::new(&path)).expect("falha ao abrir o .pkg");

    let names: Vec<&str> = pkg.names().collect();
    println!("{} arquivos no pacote:", names.len());
    for n in &names {
        println!("  {n}");
    }

    // Segundo arg opcional: imprime o conteúdo completo daquele arquivo do pkg.
    if let Some(entry) = std::env::args().nth(2) {
        match pkg.read(&entry) {
            Some(data) => println!("\n=== {entry} ({} bytes) ===\n{}", data.len(), String::from_utf8_lossy(data)),
            None => println!("\n(entrada {entry:?} não encontrada)"),
        }
        return;
    }

    if let Some(data) = pkg.read("scene.json") {
        let preview = String::from_utf8_lossy(&data[..data.len().min(200)]);
        println!("\nscene.json ({} bytes), início:\n{preview}", data.len());
    }
}
