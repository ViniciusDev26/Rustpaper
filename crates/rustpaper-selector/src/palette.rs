// A color theme from a wallpaper image, like Aether does for Omarchy: the
// image's dominant colors (k-means in OKLab) become an Omarchy v4 palette
// (colors.toml: backgrounds, foregrounds, accent, the ANSI colors).
//
// Everything is done in OKLCH (perceptual lightness / chroma / hue), so the
// roles can be given fixed lightness targets: backgrounds are tinted with
// the image's darkest dominant hue, the accent is its most vivid color, and
// each ANSI color takes the image's own hue when the image has one near it
// (otherwise the canonical hue), at a lightness that keeps it readable on
// the background. Pure: pixels in, palette out.

/// OKLCH color: lightness 0..1, chroma 0..~0.37, hue in degrees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lch {
    pub l: f32,
    pub c: f32,
    pub h: f32,
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// sRGB (0..255) -> OKLab
fn to_oklab([r, g, b]: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = [r, g, b].map(|v| srgb_to_linear(v as f32 / 255.0));
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

/// OKLab -> linear sRGB (may be out of gamut)
fn oklab_to_linear([lab_l, a, b]: [f32; 3]) -> [f32; 3] {
    let l = (lab_l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m = (lab_l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s = (lab_l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
}

fn lab_to_lch([l, a, b]: [f32; 3]) -> Lch {
    Lch {
        l,
        c: (a * a + b * b).sqrt(),
        h: b.atan2(a).to_degrees().rem_euclid(360.0),
    }
}

fn lch_to_lab(c: Lch) -> [f32; 3] {
    let h = c.h.to_radians();
    [c.l, c.c * h.cos(), c.c * h.sin()]
}

impl Lch {
    pub fn new(l: f32, c: f32, h: f32) -> Lch {
        Lch {
            l: l.clamp(0.0, 1.0),
            c: c.max(0.0),
            h: h.rem_euclid(360.0),
        }
    }

    /// `#rrggbb`. Out-of-gamut colors keep their lightness and hue and lose
    /// chroma until they fit (instead of clipping, which shifts the hue).
    pub fn hex(self) -> String {
        let fits = |c: f32| {
            oklab_to_linear(lch_to_lab(Lch { c, ..self }))
                .iter()
                .all(|v| (-1e-4..=1.0 + 1e-4).contains(v))
        };
        let mut c = self.c;
        if !fits(c) {
            let (mut lo, mut hi) = (0.0, c);
            for _ in 0..20 {
                let mid = (lo + hi) / 2.0;
                if fits(mid) { lo = mid } else { hi = mid }
            }
            c = lo;
        }
        let rgb = oklab_to_linear(lch_to_lab(Lch { c, ..self }))
            .map(|v| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8);
        format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2])
    }
}

/// Shortest angle between two hues, degrees.
fn hue_dist(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// A dominant color: its OKLCH value and its share of the image (0..1).
#[derive(Clone, Copy, Debug)]
pub struct Swatch {
    pub color: Lch,
    pub weight: f32,
}

/// k-means in OKLab over the pixels (deterministic: farthest-point seeding
/// from the mean, so the same image always gives the same theme).
pub fn dominant_colors(pixels: &[[u8; 3]], k: usize) -> Vec<Swatch> {
    let labs: Vec<[f32; 3]> = pixels.iter().map(|&p| to_oklab(p)).collect();
    if labs.is_empty() {
        return Vec::new();
    }
    let dist = |a: &[f32; 3], b: &[f32; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>();
    let n = labs.len() as f32;
    let mean = [0, 1, 2].map(|i| labs.iter().map(|p| p[i]).sum::<f32>() / n);
    let mut centers: Vec<[f32; 3]> = vec![mean];
    while centers.len() < k.min(labs.len()) {
        let far = labs
            .iter()
            .max_by(|a, b| {
                let da = centers.iter().map(|c| dist(a, c)).fold(f32::MAX, f32::min);
                let db = centers.iter().map(|c| dist(b, c)).fold(f32::MAX, f32::min);
                da.total_cmp(&db)
            })
            .copied()
            .unwrap_or(mean);
        centers.push(far);
    }
    let mut assign = vec![0usize; labs.len()];
    for _ in 0..16 {
        for (i, p) in labs.iter().enumerate() {
            assign[i] = (0..centers.len())
                .min_by(|&a, &b| dist(p, &centers[a]).total_cmp(&dist(p, &centers[b])))
                .unwrap_or(0);
        }
        for (j, c) in centers.iter_mut().enumerate() {
            let members: Vec<&[f32; 3]> = labs
                .iter()
                .zip(&assign)
                .filter(|(_, a)| **a == j)
                .map(|(p, _)| p)
                .collect();
            if !members.is_empty() {
                let m = members.len() as f32;
                *c = [0, 1, 2].map(|i| members.iter().map(|p| p[i]).sum::<f32>() / m);
            }
        }
    }
    let mut out: Vec<Swatch> = centers
        .iter()
        .enumerate()
        .map(|(j, c)| Swatch {
            color: lab_to_lch(*c),
            weight: assign.iter().filter(|a| **a == j).count() as f32 / n,
        })
        .filter(|s| s.weight > 0.0)
        .collect();
    out.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    out
}

/// An Omarchy v4 palette (the keys of colors.toml, in Aether's order).
#[derive(Clone, Debug)]
pub struct Palette {
    pub dark: bool,
    pub entries: Vec<(&'static str, Lch)>,
}

impl Palette {
    pub fn get(&self, key: &str) -> Option<Lch> {
        self.entries
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, c)| *c)
    }

    /// colors.toml contents (Omarchy generates every app's colors from it).
    pub fn to_colors_toml(&self, source: &str) -> String {
        let mut s = format!(
            "# Generated by Rustpaper for Omarchy v4 from {source}.\nmode = \"{}\"\n",
            if self.dark { "dark" } else { "light" }
        );
        for (i, (k, c)) in self.entries.iter().enumerate() {
            // blank lines between groups, like Aether's files
            if [0, 4, 8, 12, 20].contains(&i) {
                s.push('\n');
            }
            s.push_str(&format!("{k} = \"{}\"\n", c.hex()));
        }
        s
    }

    /// The Yaru icon variant closest to the accent's hue.
    pub fn icon_theme(&self) -> &'static str {
        let a = self.get("accent").unwrap_or(Lch::new(0.7, 0.1, 264.0));
        if a.c < 0.04 {
            return "Yaru";
        }
        const BY_HUE: [(f32, &str); 9] = [
            (25.0, "Yaru-red"),
            (55.0, "Yaru"),
            (90.0, "Yaru-yellow"),
            (120.0, "Yaru-olive"),
            (150.0, "Yaru-sage"),
            (195.0, "Yaru-prussiangreen"),
            (255.0, "Yaru-blue"),
            (300.0, "Yaru-purple"),
            (340.0, "Yaru-magenta"),
        ];
        BY_HUE
            .iter()
            .min_by(|x, y| hue_dist(a.h, x.0).total_cmp(&hue_dist(a.h, y.0)))
            .map(|(_, n)| *n)
            .unwrap_or("Yaru")
    }
}

/// Canonical OKLCH hues of the ANSI colors (sRGB primaries / Aether's).
const ANSI: [(&str, f32); 7] = [
    ("red", 25.0),
    ("orange", 55.0),
    ("yellow", 95.0),
    ("green", 142.0),
    ("cyan", 195.0),
    ("blue", 260.0),
    ("magenta", 320.0),
];

/// The palette for an image's pixels (any count; a downscaled image is
/// plenty).
pub fn palette_from_pixels(pixels: &[[u8; 3]]) -> Palette {
    let swatches = dominant_colors(pixels, 12);
    let significant: Vec<Swatch> = swatches
        .iter()
        .copied()
        .filter(|s| s.weight >= 0.02)
        .collect();
    let pool = if significant.is_empty() {
        &swatches
    } else {
        &significant
    };
    let mean_l = swatches.iter().map(|s| s.color.l * s.weight).sum::<f32>();
    // dark unless the image is mostly very light (dark themes read better
    // over most wallpapers, and that is what Omarchy defaults to)
    let dark = mean_l < 0.72;

    // background hue: the darkest (dark mode) / lightest (light mode)
    // significant color; its chroma tints the neutrals, kept subtle
    let base = pool
        .iter()
        .min_by(|a, b| {
            let (x, y) = if dark {
                (a.color.l, b.color.l)
            } else {
                (b.color.l, a.color.l)
            };
            x.total_cmp(&y)
        })
        .map(|s| s.color)
        .unwrap_or(Lch::new(0.2, 0.02, 264.0));
    let tint = (base.c * 0.6).clamp(0.008, 0.035);
    let neutral = |l: f32, c_scale: f32| Lch::new(l, tint * c_scale, base.h);

    // accent: the most vivid color, weighted by how much of the image it is
    let vivid = pool
        .iter()
        .filter(|s| s.color.c > 0.05)
        .max_by(|a, b| {
            (a.color.c * a.color.c * a.weight.sqrt())
                .total_cmp(&(b.color.c * b.color.c * b.weight.sqrt()))
        })
        .map(|s| s.color);
    let (accent_l, ansi_l, bright_l): (f32, f32, f32) = if dark {
        (0.76, 0.76, 0.81)
    } else {
        (0.55, 0.55, 0.50)
    };
    let accent = match vivid {
        Some(v) => Lch::new(accent_l, v.c.clamp(0.09, 0.17), v.h),
        None => Lch::new(accent_l, 0.10, (base.h + 180.0) % 360.0),
    };

    // ANSI colors: the image's own hue when it has a reasonably vivid color
    // near the canonical one (pulled halfway back, so red stays red),
    // otherwise the canonical hue
    let ansi = |target: f32, l: f32, c: f32| {
        let near = pool
            .iter()
            .filter(|s| s.color.c > 0.05 && hue_dist(s.color.h, target) < 35.0)
            .max_by(|a, b| a.weight.total_cmp(&b.weight));
        let h = match near {
            Some(s) => {
                let d = ((s.color.h - target + 540.0) % 360.0) - 180.0;
                target + d * 0.5
            }
            None => target,
        };
        Lch::new(l, c, h)
    };
    let hue_of = |name: &str| {
        ANSI.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, h)| *h)
            .unwrap_or(0.0)
    };
    let color = |name: &str| {
        // yellow reads dim at the shared lightness; blue reads too bright
        let dl = match name {
            "yellow" => 0.08,
            "blue" => -0.03,
            _ => 0.0,
        };
        ansi(
            hue_of(name),
            ansi_l + if dark { dl } else { -dl / 2.0 },
            0.13,
        )
    };
    let bright = |name: &str| {
        let c = color(name);
        Lch::new(
            if dark {
                bright_l.max(c.l)
            } else {
                bright_l.min(c.l)
            },
            0.14,
            c.h,
        )
    };

    let (bg, dark_bg, darker_bg, lighter_bg, muted) = if dark {
        (
            neutral(0.22, 1.0),
            neutral(0.18, 1.0),
            neutral(0.14, 1.0),
            neutral(0.31, 1.0),
            neutral(0.40, 0.8),
        )
    } else {
        (
            neutral(0.97, 0.6),
            neutral(0.93, 0.7),
            neutral(0.89, 0.8),
            neutral(0.99, 0.4),
            neutral(0.82, 0.8),
        )
    };
    let (fg, dark_fg, light_fg, bright_fg) = if dark {
        (
            neutral(0.90, 0.6),
            neutral(0.72, 0.6),
            neutral(0.93, 0.5),
            neutral(0.95, 0.4),
        )
    } else {
        (
            neutral(0.32, 1.2),
            neutral(0.45, 1.0),
            neutral(0.25, 1.2),
            neutral(0.20, 1.2),
        )
    };
    let orange = color("orange");
    let entries = vec![
        ("accent", accent),
        ("selection", lighter_bg),
        ("selection_foreground", bg),
        ("muted", muted),
        ("background", bg),
        ("dark_background", dark_bg),
        ("darker_background", darker_bg),
        ("lighter_background", lighter_bg),
        ("foreground", fg),
        ("dark_foreground", dark_fg),
        ("light_foreground", light_fg),
        ("bright_foreground", bright_fg),
        ("red", color("red")),
        ("yellow", color("yellow")),
        ("orange", orange),
        ("green", color("green")),
        ("cyan", color("cyan")),
        ("blue", color("blue")),
        ("magenta", color("magenta")),
        (
            "brown",
            Lch::new(if dark { 0.52 } else { 0.45 }, 0.07, orange.h),
        ),
        ("bright_red", bright("red")),
        ("bright_yellow", bright("yellow")),
        ("bright_green", bright("green")),
        ("bright_cyan", bright("cyan")),
        ("bright_blue", bright("blue")),
        ("bright_magenta", bright("magenta")),
    ];
    Palette { dark, entries }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two hex colors.
    fn contrast(a: &str, b: &str) -> f32 {
        let lum = |h: &str| {
            let v = |i: usize| {
                srgb_to_linear(u8::from_str_radix(&h[i..i + 2], 16).unwrap() as f32 / 255.0)
            };
            0.2126 * v(1) + 0.7152 * v(3) + 0.0722 * v(5)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    fn image(parts: &[([u8; 3], usize)]) -> Vec<[u8; 3]> {
        parts
            .iter()
            .flat_map(|&(c, n)| std::iter::repeat_n(c, n))
            .collect()
    }

    #[test]
    fn oklab_round_trips() {
        for c in [[255, 0, 0], [12, 200, 90], [30, 30, 46], [250, 250, 250]] {
            let back = lab_to_lch(to_oklab(c)).hex();
            assert_eq!(back, format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]));
        }
    }

    #[test]
    fn dark_night_scene_with_red_lights() {
        // mostly dark navy, some mid blue, a little vivid red
        let px = image(&[
            ([18, 22, 48], 700),
            ([40, 70, 140], 220),
            ([220, 40, 50], 80),
        ]);
        let p = palette_from_pixels(&px);
        assert!(p.dark);
        let bg = p.get("background").unwrap();
        assert!(
            hue_dist(bg.h, lab_to_lch(to_oklab([18, 22, 48])).h) < 10.0,
            "bg tinted navy"
        );
        // the accent is the vivid red
        assert!(
            hue_dist(p.get("accent").unwrap().h, 25.0) < 20.0,
            "{:?}",
            p.get("accent")
        );
        // readable text
        let hex = |k: &str| p.get(k).unwrap().hex();
        assert!(contrast(&hex("foreground"), &hex("background")) > 7.0);
        assert!(contrast(&hex("accent"), &hex("background")) > 4.5);
        for k in ["red", "yellow", "green", "cyan", "blue", "magenta"] {
            assert!(contrast(&hex(k), &hex("background")) > 4.5, "{k}");
        }
    }

    #[test]
    fn very_light_image_gives_a_light_theme() {
        let px = image(&[([245, 240, 230], 900), ([200, 120, 90], 100)]);
        let p = palette_from_pixels(&px);
        assert!(!p.dark);
        let hex = |k: &str| p.get(k).unwrap().hex();
        assert!(contrast(&hex("foreground"), &hex("background")) > 7.0);
    }

    #[test]
    fn colors_toml_has_omarchy_keys() {
        let px = image(&[([10, 10, 10], 10)]);
        let toml = palette_from_pixels(&px).to_colors_toml("test");
        for key in [
            "mode",
            "accent",
            "background",
            "foreground",
            "bright_magenta",
            "brown",
        ] {
            assert!(
                toml.contains(&format!("\n{key} = \"")),
                "{key} missing in\n{toml}"
            );
        }
        // a grey image still gets a full, valid palette
        assert_eq!(toml.matches(" = \"#").count(), 26);
    }
}
