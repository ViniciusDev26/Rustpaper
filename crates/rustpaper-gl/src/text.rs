// Text objects: rasterized on the CPU into a texture that is then drawn like
// any image layer (so color, alpha and effects on text work unchanged).
//
// No SceneScript is ever executed (see docs/GL_RENDERER.md). Text driven by a
// script shows its serialized value, EXCEPT for the clock/date presets that
// are recognizable from their data (`scriptproperties`), which are implemented
// natively here:
//   - "Digital clock": use24hFormat, showSeconds, delimiter -> HH:MM[:SS]
//   - format string: format ("yyyy/MM/dd hh:mm", tokens below), twelve, week
//   - "Digital date": dayFormat, monthFormat, showDay, useDelimiter,
//     addDelimiter, alignVertical
// The formats mirror those presets' published code.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont, point};
use chrono::{Datelike, Local, Timelike};
use serde_json::Value;

use crate::value;

/// Font size in scene units per `pointsize` point. Measured against WE on a
/// 1080p and a 4K scene (Colored star, Yor): digits are ~0.7 em tall and the
/// em is ~4.15 units per point in both (Alcubierre: 93 px digits at 32 pt).
pub const UNITS_PER_POINT: f32 = 4.15;

#[derive(Clone, Debug, PartialEq)]
pub enum TextSource {
    Static(String),
    Clock {
        h24: bool,
        seconds: bool,
        delimiter: String,
    },
    /// "Time and Date" preset: `[AM|PM ]HH:MM` + newline + `Mmm. D YYYY`.
    TimeAndDate {
        h24: bool,
        delimiter: String,
    },
    Format {
        format: String,
        twelve: bool,
        week: String,
    },
    DigitalDate {
        weekday_only: bool,
        show_day: bool,
        month: u32,
        day: u32,
        delimiter: String,
        vertical: bool,
    },
}

impl TextSource {
    /// Reads the `text` field of a text object.
    pub fn from_json(text: &Value) -> TextSource {
        let v = value::plain(text);
        let shown = match v {
            Value::String(s) => s.clone(),
            other => value::plain(other.get("value").unwrap_or(&Value::Null))
                .as_str()
                .unwrap_or("")
                .to_string(),
        };
        let Some(props) = text.get("scriptproperties").filter(|p| p.is_object()) else {
            return TextSource::Static(shown);
        };
        let s = |k: &str| {
            value::plain(props.get(k).unwrap_or(&Value::Null))
                .as_str()
                .map(String::from)
        };
        let b = |k: &str, d: bool| value::get_bool(props, k, d);
        if shown == "<Time and Date>" {
            return TextSource::TimeAndDate {
                h24: b("use24hFormat", true),
                delimiter: s("delimiter").unwrap_or_else(|| ":".into()),
            };
        }
        if props.get("use24hFormat").is_some() {
            return TextSource::Clock {
                h24: b("use24hFormat", true),
                seconds: b("showSeconds", false),
                delimiter: s("delimiter").unwrap_or_else(|| ":".into()),
            };
        }
        if let Some(format) = s("format") {
            return TextSource::Format {
                format,
                twelve: b("twelve", false),
                week: s("week").unwrap_or_else(|| "long".into()),
            };
        }
        if props.get("dayFormat").is_some() && props.get("monthFormat").is_some() {
            let num = |k: &str| s(k).and_then(|v| v.parse().ok()).unwrap_or(1);
            return TextSource::DigitalDate {
                weekday_only: shown == "DAY" && b("showDay", true),
                show_day: b("showDay", false),
                month: num("monthFormat"),
                day: num("dayFormat"),
                delimiter: if b("useDelimiter", false) {
                    s("addDelimiter").unwrap_or_else(|| "/".into())
                } else {
                    " ".into()
                },
                vertical: b("alignVertical", false),
            };
        }
        TextSource::Static(shown)
    }

    /// The text to show right now.
    pub fn current(&self) -> String {
        let now = clock_now();
        match self {
            TextSource::Static(s) => s.clone(),
            TextSource::Clock {
                h24,
                seconds,
                delimiter,
            } => {
                let h = if *h24 {
                    now.hour()
                } else {
                    (now.hour() + 11) % 12 + 1
                };
                let mut out = format!("{h:02}{delimiter}{:02}", now.minute());
                if *seconds {
                    out.push_str(&format!("{delimiter}{:02}", now.second()));
                }
                out
            }
            TextSource::TimeAndDate { h24, delimiter } => {
                const MONTHS: [&str; 12] = [
                    "Jan.", "Feb.", "Mar.", "Apr.", "May.", "Jun.", "Jul.", "Aug.", "Sep.", "Oct.",
                    "Nov.", "Dec.",
                ];
                let (h, prefix) = if *h24 {
                    (now.hour(), String::new())
                } else {
                    let m = if now.hour() < 12 { "AM " } else { "PM " };
                    ((now.hour() + 11) % 12 + 1, m.to_string())
                };
                format!(
                    "{prefix}{h:02}{delimiter}{:02}\n{} {} {}",
                    now.minute(),
                    MONTHS[now.month0() as usize],
                    now.day(),
                    now.year()
                )
            }
            TextSource::Format {
                format,
                twelve,
                week,
            } => format_tokens(format, *twelve, week, &now),
            TextSource::DigitalDate {
                weekday_only,
                show_day,
                month,
                day,
                delimiter,
                vertical,
            } => {
                const DAYS: [&str; 7] = [
                    "Sunday",
                    "Monday",
                    "Tuesday",
                    "Wednesday",
                    "Thursday",
                    "Friday",
                    "Saturday",
                ];
                const MONTHS: [&str; 12] = [
                    "January",
                    "February",
                    "March",
                    "April",
                    "May",
                    "June",
                    "July",
                    "August",
                    "September",
                    "October",
                    "November",
                    "December",
                ];
                let wd = now.weekday().num_days_from_sunday() as usize;
                let m = now.month0() as usize;
                if *weekday_only {
                    // the preset spaces the letters: "M O N D A Y"
                    let name = if *day == 1 { &DAYS[wd][..3] } else { DAYS[wd] };
                    return name
                        .to_uppercase()
                        .chars()
                        .map(String::from)
                        .collect::<Vec<_>>()
                        .join(" ");
                }
                let month_s = match month {
                    1 => (m + 1).to_string(),
                    2 => format!(" {} ", MONTHS[m][..3].to_uppercase()),
                    _ => MONTHS[m].to_string(),
                };
                let date = format!("{}{delimiter}{month_s}{delimiter}{}", now.day(), now.year());
                if *show_day {
                    // the preset's day names (note its Saturday quirk: "\nSat")
                    let mut name = if *day == 1 {
                        DAYS[wd][..3].to_string()
                    } else {
                        DAYS[wd].to_string()
                    };
                    if wd == 6 {
                        name.insert(0, '\n');
                    }
                    let nl = if *vertical { "\n" } else { "" };
                    format!("{name}{nl} {date}")
                } else {
                    date
                }
            }
        }
    }
}

/// yyyy yy MM M dd d hh h mm m ss s S a W, as in the "format" clock preset.
/// Wall clock for clock/date text. `RUSTPAPER_CLOCK` (RFC 3339, e.g.
/// `2026-11-08T21:22:05-03:00`) pins the clock's start: the harness sets it
/// to the moment the WE capture started, so both show the same time.
pub fn clock_now() -> chrono::DateTime<Local> {
    use std::sync::OnceLock;
    static OFFSET: OnceLock<chrono::TimeDelta> = OnceLock::new();
    let offset = OFFSET.get_or_init(|| {
        std::env::var("RUSTPAPER_CLOCK")
            .ok()
            .and_then(|v| chrono::DateTime::parse_from_rfc3339(v.trim()).ok())
            .map(|t| t.with_timezone(&Local) - Local::now())
            .unwrap_or_default()
    });
    Local::now() + *offset
}

/// Fraction of the local day in [0,1) on that clock (`g_Daytime`,
/// `engine.timeOfDay`). The harness pins it to the capture's start: WE
/// captured Persona 3 3151551777's day/night cycle at 17:08 (0.71), and we
/// rendered it at a fixed noon (0.5): a blue sky instead of WE's evening.
pub fn daytime() -> f32 {
    let t = clock_now();
    let secs = t.num_seconds_from_midnight() as f32 + t.nanosecond() as f32 * 1e-9;
    (secs / 86_400.0).clamp(0.0, 0.999_999)
}

fn format_tokens(format: &str, twelve: bool, week: &str, now: &chrono::DateTime<Local>) -> String {
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let hour = if twelve {
        (now.hour() + 11) % 12 + 1
    } else {
        now.hour()
    };
    let mut out = String::new();
    let chars: Vec<char> = format.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' && i + 1 < chars.len() {
            out.push(chars[i + 1]);
            i += 2;
            continue;
        }
        let mut n = 1;
        while i + n < chars.len() && chars[i + n] == c {
            n += 1;
        }
        let two = |v: u32| {
            if n >= 2 {
                format!("{v:02}")
            } else {
                v.to_string()
            }
        };
        let piece = match c {
            'y' if n >= 4 => now.year().to_string(),
            'y' => format!("{:02}", now.year() % 100),
            'M' => two(now.month()),
            'd' => two(now.day()),
            'h' => two(hour),
            'm' => two(now.minute()),
            's' => two(now.second()),
            'S' => format!("{:03}", now.timestamp_subsec_millis())[..n.min(3)].to_string(),
            'a' if twelve => (if now.hour() < 12 { "AM" } else { "PM" }).to_string(),
            'W' => {
                let d = DAYS[now.weekday().num_days_from_sunday() as usize];
                match week {
                    "narrow" => d[..1].to_string(),
                    "short" => d[..3].to_string(),
                    _ => d.to_string(),
                }
            }
            _ => std::iter::repeat_n(c, n).collect(),
        };
        out.push_str(&piece);
        i += n;
    }
    out
}

/// Layout parameters of a text object (scene units).
#[derive(Clone, Debug)]
pub struct TextStyle {
    pub em: f32,
    pub halign: String,
    pub valign: String,
    pub padding: f32,
    pub size: (u32, u32),
    /// opaque background (rgb 0..1), when `opaquebackground`
    pub background: Option<[f32; 3]>,
}

/// A text object's font plus fallbacks for characters it lacks (CJK in a
/// Latin font: WE, through Windows font linking, draws them; we drew boxes
/// for 3832902426's "星期日").
pub struct FontSet {
    fonts: Vec<FontVec>,
    paths: Vec<std::path::PathBuf>,
    /// Characters no system font has (don't ask fontconfig again).
    missing: std::collections::HashSet<char>,
}

impl FontSet {
    pub fn new(primary: FontVec) -> FontSet {
        FontSet {
            fonts: vec![primary],
            paths: Vec::new(),
            missing: Default::default(),
        }
    }

    /// Index of the first font with a glyph for `c` (0 when none has it).
    fn pick(&self, c: char) -> usize {
        self.fonts
            .iter()
            .position(|f| f.glyph_id(c).0 != 0)
            .unwrap_or(0)
    }

    /// Loads fallback fonts (fontconfig `:charset=`) for the characters of
    /// `text` that no loaded font covers.
    pub fn cover(&mut self, text: &str) {
        for c in text.chars() {
            if c.is_control()
                || c.is_whitespace()
                || self.missing.contains(&c)
                || self.fonts.iter().any(|f| f.glyph_id(c).0 != 0)
            {
                continue;
            }
            let found = std::process::Command::new("fc-match")
                .args(["-f", "%{file}", &format!(":charset={:x}", c as u32)])
                .output()
                .ok()
                .map(|o| std::path::PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
                .filter(|p| p.is_file() && !self.paths.contains(p));
            let font = found.as_ref().and_then(|p| {
                let bytes = std::fs::read(p).ok()?;
                FontVec::try_from_vec(bytes).ok()
            });
            match (found, font) {
                (Some(p), Some(f)) if f.glyph_id(c).0 != 0 => {
                    self.paths.push(p);
                    self.fonts.push(f);
                }
                _ => {
                    self.missing.insert(c);
                }
            }
        }
    }

    fn scale(&self, k: usize, em: f32) -> PxScale {
        let f = &self.fonts[k];
        let upem = f.units_per_em().unwrap_or(1000.0);
        PxScale::from(em * f.height_unscaled() / upem)
    }

    /// Glyphs of one line: (font, glyph, x) and the line's width.
    fn layout(&self, line: &str, em: f32) -> (Vec<(usize, ab_glyph::GlyphId, f32)>, f32) {
        let mut out = Vec::new();
        let mut x = 0.0;
        let mut prev: Option<(usize, ab_glyph::GlyphId)> = None;
        for c in line.chars() {
            let k = self.pick(c);
            let sf = self.fonts[k].as_scaled(self.scale(k, em));
            let g = sf.glyph_id(c);
            if let Some((pk, pg)) = prev
                && pk == k
            {
                x += sf.kern(pg, g);
            }
            out.push((k, g, x));
            x += sf.h_advance(g);
            prev = Some((k, g));
        }
        (out, x)
    }
}

/// Canvas needed for `text`: the object's box, grown to fit the text (WE
/// lets text overflow its box; the box only anchors the alignment).
pub fn canvas_size(fonts: &FontSet, text: &str, style: &TextStyle) -> (u32, u32) {
    let sf = fonts.fonts[0].as_scaled(fonts.scale(0, style.em));
    let lines: Vec<&str> = text.split('\n').collect();
    let w = lines
        .iter()
        .map(|l| fonts.layout(l, style.em).1)
        .fold(0.0f32, f32::max);
    let h = (sf.height() + sf.line_gap()) * lines.len() as f32 - sf.line_gap();
    let pad = 2.0 * style.padding;
    (
        style.size.0.max((w + pad).ceil() as u32 + 2).min(8192),
        style.size.1.max((h + pad).ceil() as u32 + 2).min(8192),
    )
}

/// Rasterizes `text` into an RGBA buffer of `style.size`: white glyphs with
/// coverage as alpha (the layer's color tints them), over the background.
pub fn rasterize(fonts: &FontSet, text: &str, style: &TextStyle) -> Vec<u8> {
    let (w, h) = style.size;
    let mut px = vec![0u8; (w * h * 4) as usize];
    if let Some(bg) = style.background {
        for p in px.chunks_mut(4) {
            p.copy_from_slice(&[
                (bg[0] * 255.0) as u8,
                (bg[1] * 255.0) as u8,
                (bg[2] * 255.0) as u8,
                255,
            ]);
        }
    }
    // line metrics come from the object's own font
    let sf = fonts.fonts[0].as_scaled(fonts.scale(0, style.em));
    let lines: Vec<&str> = text.split('\n').collect();
    let line_h = sf.height() + sf.line_gap();
    let total_h = line_h * lines.len() as f32 - sf.line_gap();
    let pad = style.padding;
    let (bw, bh) = (w as f32 - 2.0 * pad, h as f32 - 2.0 * pad);
    let top = pad
        + match style.valign.as_str() {
            "top" => 0.0,
            "bottom" => bh - total_h,
            _ => (bh - total_h) / 2.0,
        };
    for (li, line) in lines.iter().enumerate() {
        let (glyphs, lw) = fonts.layout(line, style.em);
        let x0 = pad
            + match style.halign.as_str() {
                "left" => 0.0,
                "right" => bw - lw,
                _ => (bw - lw) / 2.0,
            };
        let baseline = top + li as f32 * line_h + sf.ascent();
        for (k, gid, gx) in glyphs {
            let glyph =
                gid.with_scale_and_position(fonts.scale(k, style.em), point(x0 + gx, baseline));
            let Some(outline) = fonts.fonts[k].outline_glyph(glyph) else {
                continue;
            };
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let (xx, yy) = (bb.min.x as i32 + gx as i32, bb.min.y as i32 + gy as i32);
                if xx < 0 || yy < 0 || xx >= w as i32 || yy >= h as i32 {
                    return;
                }
                let i = ((yy as u32 * w + xx as u32) * 4) as usize;
                let a = (cov.clamp(0.0, 1.0) * 255.0) as u8;
                if style.background.is_some() {
                    // glyphs over an opaque background: blend toward white
                    for k in 0..3 {
                        px[i + k] = (px[i + k] as f32 + (255.0 - px[i + k] as f32) * cov) as u8;
                    }
                } else {
                    let alpha = a.max(px[i + 3]);
                    px[i..i + 4].copy_from_slice(&[255, 255, 255, alpha]);
                }
            });
        }
    }
    px
}

/// Font bytes for a text object's `font`: a file from the scene/base assets
/// ("fonts/x.ttf"), or a system font (`systemfont_arial`...) via fontconfig.
pub fn system_font_path(name: &str) -> Option<std::path::PathBuf> {
    let family = match name.trim_start_matches("systemfont_") {
        "arial" => "Arial",
        "segoe" => "Segoe UI",
        "consolas" => "Consolas:monospace",
        "times" | "timesnewroman" => "Times New Roman",
        "verdana" => "Verdana",
        "tahoma" => "Tahoma",
        other => other,
    };
    let out = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", family])
        .output()
        .ok()?;
    let p = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognizes_presets_from_data() {
        let clock = json!({ "value": "12:34", "script": "...", "scriptproperties": { "use24hFormat": { "user": "x", "value": false }, "showSeconds": true, "delimiter": "." } });
        assert_eq!(
            TextSource::from_json(&clock),
            TextSource::Clock {
                h24: false,
                seconds: true,
                delimiter: ".".into()
            }
        );
        let fmt = json!({ "value": "2021/01/01", "scriptproperties": { "format": "yyyy/MM/dd", "twelve": false, "week": "short" } });
        assert!(matches!(
            TextSource::from_json(&fmt),
            TextSource::Format { .. }
        ));
        let tad = json!({ "value": "<Time and Date>", "scriptproperties": { "delimiter": ":", "use24hFormat": true } });
        assert!(TextSource::from_json(&tad).current().contains('\n'));
        let date = json!({ "value": "DAY", "scriptproperties": { "dayFormat": "2", "monthFormat": "1", "showDay": true, "useDelimiter": false, "addDelimiter": "/", "alignVertical": false } });
        assert!(matches!(
            TextSource::from_json(&date),
            TextSource::DigitalDate {
                weekday_only: true,
                ..
            }
        ));
        // unknown script: the serialized value, never the script
        let song = json!({ "value": "Song Name", "script": "export function update() {}" });
        assert_eq!(
            TextSource::from_json(&song),
            TextSource::Static("Song Name".into())
        );
    }

    #[test]
    fn format_tokens_work() {
        let now = Local::now();
        let out = format_tokens("yyyy/MM/dd", false, "long", &now);
        assert_eq!(
            out,
            format!("{}/{:02}/{:02}", now.year(), now.month(), now.day())
        );
        assert_eq!(format_tokens("\\h", false, "long", &now), "h");
    }
}
