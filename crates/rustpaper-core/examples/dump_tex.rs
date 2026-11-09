// `cargo run -p rustpaper-core --example dump_tex -- <file.tex>`
// Decodes a standalone .tex and prints the dimensions (validates the decoder).

use rustpaper_core::tex;

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_tex <file.tex>");
    let bytes = std::fs::read(&path).expect("failed to read the file");
    match tex::parse(&bytes) {
        Ok(t) => {
            println!(
                "OK: buffer {}x{}, content {}x{}, rgba {} bytes",
                t.width,
                t.height,
                t.real_width,
                t.real_height,
                t.rgba.len()
            );
            // Optional 2nd arg: saves a PNG for visual inspection.
            if let Some(out) = std::env::args().nth(2) {
                image::save_buffer(&out, &t.rgba, t.width, t.height, image::ColorType::Rgba8)
                    .unwrap();
                println!("PNG saved to {out}");
            }
        }
        Err(e) => println!("ERROR: {e}"),
    }
}
