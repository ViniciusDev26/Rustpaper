// Prints a puppet model summary: cargo run -p rustpaper-core --example dump_mdl -- <file.mdl>
fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_mdl <file.mdl>");
    let d = std::fs::read(&path).expect("read");
    match rustpaper_core::mdl::Puppet::parse(&d) {
        Ok(p) => {
            println!(
                "material {} | {} vertices, {} triangles, {} bones",
                p.material,
                p.mesh.positions.len(),
                p.mesh.indices.len() / 3,
                p.bones.len()
            );
            for a in &p.animations {
                println!(
                    "  animation #{} {:?} {:?} rate {} frames {} tracks {}x{}",
                    a.id,
                    a.name,
                    a.mode,
                    a.fps,
                    a.frames,
                    a.tracks.len(),
                    a.tracks.first().map_or(0, |t| t.len())
                );
            }
        }
        Err(e) => println!("error: {e}"),
    }
}
