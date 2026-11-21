// Prints a font's vertical metrics (used to calibrate text sizing vs WE).
// cargo run -p rustpaper-gl --example font_metrics -- <font file>
use ab_glyph::{Font, FontVec};
fn main() {
    let path = std::env::args().nth(1).expect("font file");
    let font = FontVec::try_from_vec(std::fs::read(&path).unwrap()).unwrap();
    let upem = font.units_per_em().unwrap_or(0.0);
    let h = font.height_unscaled();
    let digit = font
        .outline(font.glyph_id('2'))
        .map(|o| o.bounds.max.y - o.bounds.min.y);
    println!(
        "{path}: upem={upem} height={h} height/upem={:.3} digit/upem={:?}",
        h / upem,
        digit.map(|d| d / upem)
    );
}
