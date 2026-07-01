// `cargo run --example dump_pkg -- <caminho-do-scene.pkg>`
// Lista os arquivos dentro de um .pkg do Wallpaper Engine e mostra o começo do
// scene.json (se houver) — validação contra dados reais.

#[path = "../src/pkg.rs"]
mod pkg;

use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("uso: dump_pkg <arquivo.pkg>");
    let pkg = pkg::Pkg::open(Path::new(&path)).expect("falha ao abrir o .pkg");

    let names: Vec<&str> = pkg.names().collect();
    println!("{} arquivos no pacote:", names.len());
    for n in &names {
        println!("  {n}");
    }

    if let Some(data) = pkg.read("scene.json") {
        let preview: String = String::from_utf8_lossy(&data[..data.len().min(200)]).into_owned();
        println!("\nscene.json ({} bytes), início:\n{preview}", data.len());
    }
}
