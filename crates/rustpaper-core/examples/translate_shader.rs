// Dev tool: translates a WE shader to GLSL and prints it to stdout.
// Usage: cargo run -p rustpaper-core --example translate_shader -- <vert|frag> <name> [shaders_dir]
// E.g.: ... translate_shader frag genericimage2 /home/vscode/we-assets/shaders
use rustpaper_core::shader::{Stage, translate};
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stage = match args.get(1).map(String::as_str) {
        Some("vert") => Stage::Vertex,
        Some("frag") => Stage::Fragment,
        _ => {
            eprintln!("usage: translate_shader <vert|frag> <name> [shaders_dir] [COMBO=value ...]");
            std::process::exit(2);
        }
    };
    let name = args.get(2).expect("shader name");
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
    let src = std::fs::read_to_string(dir.join(format!("{name}.{ext}"))).expect("read shader");

    // combos extra via CLI: NAME=value
    let combos: Vec<(String, i64)> = args[4.min(args.len())..]
        .iter()
        .filter_map(|a| {
            let (k, v) = a.split_once('=')?;
            Some((k.to_string(), v.parse().ok()?))
        })
        .collect();

    print!(
        "{}",
        translate(stage, &src, &combos, &dir).expect("translate")
    );
}
