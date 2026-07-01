// `cargo run -p we-core --example dump_tex -- <arquivo.tex>`
// Decodifica um .tex avulso e imprime as dimensões (valida o decoder).

use we_core::tex;

fn main() {
    let path = std::env::args().nth(1).expect("uso: dump_tex <arquivo.tex>");
    let bytes = std::fs::read(&path).expect("falha ao ler o arquivo");
    match tex::parse(&bytes) {
        Ok(t) => {
            println!(
                "OK: buffer {}x{}, conteúdo {}x{}, rgba {} bytes",
                t.width, t.height, t.real_width, t.real_height, t.rgba.len()
            );
        }
        Err(e) => println!("ERRO: {e}"),
    }
}
