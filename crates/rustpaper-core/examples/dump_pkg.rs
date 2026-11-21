// `cargo run -p rustpaper-core --example dump_pkg -- <file.pkg>`
// Lists the files of a Wallpaper Engine .pkg.

use rustpaper_core::pkg::Pkg;
use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_pkg <file.pkg>");
    let pkg = Pkg::open(Path::new(&path)).expect("failed to open the .pkg");

    let names: Vec<&str> = pkg.names().collect();
    println!("{} files in the package:", names.len());
    for n in &names {
        println!("  {n}");
    }

    // Optional second arg: prints the full contents of that file inside the pkg.
    if let Some(entry) = std::env::args().nth(2) {
        match pkg.read(&entry) {
            Some(data) => println!(
                "\n=== {entry} ({} bytes) ===\n{}",
                data.len(),
                String::from_utf8_lossy(data)
            ),
            None => println!("\n(entry {entry:?} not found)"),
        }
        return;
    }

    if let Some(data) = pkg.read("scene.json") {
        let preview = String::from_utf8_lossy(&data[..data.len().min(200)]);
        println!("\nscene.json ({} bytes), start:\n{preview}", data.len());
    }
}
