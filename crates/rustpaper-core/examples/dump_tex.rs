// `cargo run -p rustpaper-core --example dump_tex -- <arquivo.tex>`
// Decodifica um .tex avulso e imprime as dimensões (valida o decoder).

use rustpaper_core::tex;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("uso: dump_tex <arquivo.tex>");
    let bytes = std::fs::read(&path).expect("falha ao ler o arquivo");
    match tex::parse(&bytes) {
        Ok(t) => {
            println!(
                "OK: buffer {}x{}, conteúdo {}x{}, rgba {} bytes",
                t.width,
                t.height,
                t.real_width,
                t.real_height,
                t.rgba.len()
            );
            // 2º arg opcional: salva PNG pra inspeção visual.
            if let Some(out) = std::env::args().nth(2) {
                image::save_buffer(&out, &t.rgba, t.width, t.height, image::ColorType::Rgba8)
                    .unwrap();
                println!("PNG salvo em {out}");
            }
        }
        Err(e) => println!("ERRO: {e}"),
    }
}
