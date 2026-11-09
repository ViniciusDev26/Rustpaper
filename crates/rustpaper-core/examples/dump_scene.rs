// `cargo run -p rustpaper-core --example dump_scene -- <scene.pkg>`
// Opens the pkg, finds the background texture (scene.json -> model -> material) and
// decodes it (.tex -> RGBA), printing the dimensions.

use rustpaper_core::{pkg::Pkg, scene, tex};
use std::path::Path;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: dump_scene <scene.pkg>");
    let pkg = Pkg::open(Path::new(&path)).expect("failed to open pkg");

    let tex_path = scene::background_texture(&pkg).expect("background texture not found");
    println!("background texture: {tex_path}");

    let bytes = pkg.read(&tex_path).expect("texture is not in the pkg");
    println!(".tex: {} bytes", bytes.len());

    match tex::parse(bytes) {
        Ok(t) => {
            println!(
                "decoded: buffer {}x{}, content {}x{}, rgba {} bytes",
                t.width,
                t.height,
                t.real_width,
                t.real_height,
                t.rgba.len()
            );
            assert_eq!(t.rgba.len(), (t.width * t.height * 4) as usize);
            println!("OK ✓ (rgba matches width*height*4)");
        }
        Err(e) => println!("ERROR: {e}"),
    }

    let particles = scene::particle_systems(&pkg);
    println!("\n{} particle system(s):", particles.len());
    for p in &particles {
        println!(
            "  texture={:?} maxcount={} rate={} origin={:?} lifetime={:?}",
            p.texture, p.system.max_count, p.system.rate, p.system.origin, p.system.lifetime
        );
    }
}
