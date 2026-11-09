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
            for (i, b) in p.bones.iter().enumerate() {
                println!(
                    "  bone {i} parent {:?} bind translation {:?}",
                    b.parent,
                    &b.local[12..15]
                );
            }
            for a in &p.attachments {
                println!(
                    "  attachment {:?} on bone {} at {:?}",
                    a.name,
                    a.bone,
                    &a.local[12..14]
                );
            }
            let verbose = std::env::var_os("MDL_KEYS").is_some();
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
                if verbose {
                    for (b, t) in a.tracks.iter().enumerate() {
                        for k in t.iter().step_by((t.len() / 6).max(1)) {
                            println!(
                                "    bone {b} pos {:?} rot {:?} scale {:?}",
                                k.position, k.rotation, k.scale
                            );
                        }
                    }
                }
            }
        }
        Err(e) => println!("error: {e}"),
    }
}
