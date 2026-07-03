// Varre um wallpaper: imprime o shader do fundo e, pra cada efeito do fundo, se é
// SIMPLES (1 pass, sem FBO) ou complexo. Ajuda a achar cenas boas pra demo.
// Uso: scan_effects <pasta-do-wallpaper>
use std::path::Path;
use rustpaper_core::{effects, pkg::Pkg, scene};

fn main() {
    let dir = std::env::args().nth(1).expect("uso: scan_effects <pasta>");
    let Ok(pkg) = Pkg::open(Path::new(&dir).join("scene.pkg").as_path()) else {
        return;
    };
    let Some(scene_json) = pkg.read("scene.json").and_then(|b| std::str::from_utf8(b).ok()) else {
        return;
    };
    let shader = scene::background_material(&pkg).map(|m| m.shader).unwrap_or_else(|| "?".into());
    let fx = effects::background_effects(scene_json);
    if fx.is_empty() {
        return;
    }
    let mut simple = 0;
    let mut complex = 0;
    let mut kinds = Vec::new();
    for e in &fx {
        if !e.visible {
            continue;
        }
        let def = pkg
            .read(&e.file)
            .and_then(|b| std::str::from_utf8(b).ok())
            .and_then(effects::parse_effect);
        match def {
            Some(d) if effects::is_simple(&d) => {
                simple += 1;
                kinds.push(format!("simple:{}", e.file));
            }
            Some(_) => {
                complex += 1;
                kinds.push(format!("complex:{}", e.file));
            }
            None => {}
        }
    }
    println!(
        "{}  bg={shader}  efeitos: {} (simple={simple} complex={complex})  {:?}",
        Path::new(&dir).file_name().unwrap().to_string_lossy(),
        fx.len(),
        kinds
    );
}
