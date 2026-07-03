// Ferramenta de dev: traduz um shader do WE pra GLSL e imprime no stdout.
// Uso: cargo run -p rustpaper-core --example translate_shader -- <vert|frag> <nome> [shaders_dir]
// Ex.: ... translate_shader frag genericimage2 /home/vscode/we-assets/shaders
use rustpaper_core::shader::{Stage, translate};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stage = match args.get(1).map(String::as_str) {
        Some("vert") => Stage::Vertex,
        Some("frag") => Stage::Fragment,
        _ => {
            eprintln!("uso: translate_shader <vert|frag> <nome> [shaders_dir] [COMBO=valor ...]");
            std::process::exit(2);
        }
    };
    let name = args.get(2).expect("nome do shader");
    let dir = PathBuf::from(
        args.get(3)
            .cloned()
            .unwrap_or_else(|| "/home/vscode/we-assets/shaders".into()),
    );
    let ext = if stage == Stage::Vertex {
        "vert"
    } else {
        "frag"
    };
    let src = std::fs::read_to_string(dir.join(format!("{name}.{ext}"))).expect("ler shader");

    // combos extras via CLI: NOME=valor
    let combos: Vec<(String, i64)> = args[4.min(args.len())..]
        .iter()
        .filter_map(|a| {
            let (k, v) = a.split_once('=')?;
            Some((k.to_string(), v.parse().ok()?))
        })
        .collect();

    print!(
        "{}",
        translate(stage, &src, &combos, &dir).expect("traduzir")
    );
}
