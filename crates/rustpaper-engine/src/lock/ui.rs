//! The lock screen's panel: clock, date, password field and status line,
//! drawn on the CPU (tiny-skia for shapes, ab_glyph for text) into a small
//! shm buffer that sits on top of the live wallpaper as a subsurface.
//!
//! Only the panel is drawn by us, and only when it changes (a key, the
//! minute, a PAM answer): the wallpaper itself is WE's buffers, untouched.
//! A 560x360 panel at scale 2 is 3 MB, redrawn in about a millisecond.

use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect, Stroke, Transform};

use super::adapter::Theme;

/// Panel size in logical pixels.
pub const PANEL: (u32, u32) = (560, 360);

/// What the password field shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    Failed(String),
}

pub struct PanelState<'a> {
    pub time: &'a str,
    pub date: &'a str,
    /// Characters typed (shown as dots).
    pub typed: usize,
    pub status: &'a Status,
    pub caps_lock: bool,
}

pub struct Panel {
    font: Option<FontVec>,
    theme: Theme,
}

impl Panel {
    pub fn new(theme: Theme) -> Self {
        let font = load_font(theme.font.as_deref());
        if font.is_none() {
            eprintln!("lock: no font found (fc-match); the panel shows no text");
        }
        Panel { font, theme }
    }

    /// Panel size in pixels at `scale`.
    pub fn pixel_size(scale: f32) -> (u32, u32) {
        (
            (PANEL.0 as f32 * scale).round() as u32,
            (PANEL.1 as f32 * scale).round() as u32,
        )
    }

    /// Draws the panel at `scale`: premultiplied RGBA.
    pub fn draw(&self, st: &PanelState, scale: f32) -> Option<Pixmap> {
        let (w, h) = Self::pixel_size(scale);
        let mut pm = Pixmap::new(w, h)?;
        let s = |v: f32| v * scale;
        let fg = self.theme.foreground;
        let shadow = [0, 0, 0];
        let cx = w as f32 / 2.0;

        if let Some(font) = &self.font {
            // a soft shadow keeps the text readable on any wallpaper
            for (dy, color, alpha) in [(s(2.0), shadow, 0.45), (0.0, fg, 1.0)] {
                text(
                    &mut pm,
                    font,
                    s(104.0),
                    st.time,
                    cx,
                    s(122.0) + dy,
                    color,
                    alpha,
                );
                text(
                    &mut pm,
                    font,
                    s(24.0),
                    st.date,
                    cx,
                    s(166.0) + dy,
                    color,
                    alpha * 0.9,
                );
            }
        }

        // the field
        let (fw, fh) = (s(380.0), s(58.0));
        let (fx, fy) = (cx - fw / 2.0, s(200.0));
        let border = match st.status {
            Status::Failed(_) => self.theme.error,
            Status::Checking => self.theme.foreground,
            Status::Idle if st.typed > 0 => self.theme.accent,
            Status::Idle => mix(self.theme.foreground, self.theme.background, 0.6),
        };
        if let Some(path) = rounded(fx, fy, fw, fh, fh / 2.0) {
            pm.fill_path(
                &path,
                &paint(self.theme.background, 0.82),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
            let stroke = Stroke {
                width: s(2.0),
                ..Default::default()
            };
            pm.stroke_path(
                &path,
                &paint(border, 1.0),
                &stroke,
                Transform::identity(),
                None,
            );
        }
        let mid = fy + fh / 2.0;
        match (st.status, st.typed) {
            (Status::Checking, _) => {
                if let Some(font) = &self.font {
                    text(
                        &mut pm,
                        font,
                        s(20.0),
                        "Checking…",
                        cx,
                        mid + s(7.0),
                        fg,
                        0.8,
                    );
                }
            }
            (_, 0) => {
                if let Some(font) = &self.font {
                    text(
                        &mut pm,
                        font,
                        s(20.0),
                        "Password",
                        cx,
                        mid + s(7.0),
                        fg,
                        0.45,
                    );
                }
            }
            (_, n) => {
                // one dot per character, up to what fits
                let n = n.min(16);
                let gap = s(20.0);
                let x0 = cx - gap * (n as f32 - 1.0) / 2.0;
                for i in 0..n {
                    if let Some(c) = PathBuilder::from_circle(x0 + gap * i as f32, mid, s(6.0)) {
                        pm.fill_path(
                            &c,
                            &paint(fg, 1.0),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
            }
        }

        // the status line
        if let Some(font) = &self.font {
            let line = match st.status {
                Status::Failed(m) => Some((m.as_str(), self.theme.error)),
                _ if st.caps_lock => Some(("Caps Lock is on", fg)),
                _ => None,
            };
            if let Some((msg, color)) = line {
                for (dy, c, a) in [(s(1.5), shadow, 0.5), (0.0, color, 1.0)] {
                    text(&mut pm, font, s(19.0), msg, cx, s(300.0) + dy, c, a);
                }
            }
        }
        Some(pm)
    }
}

fn paint(rgb: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(
        rgb[0],
        rgb[1],
        rgb[2],
        (alpha * 255.0) as u8,
    ));
    p.anti_alias = true;
    p
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let m = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    Rect::from_xywh(x, y, w, h)?;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Draws `s` centered on `cx` with its baseline at `baseline`.
#[allow(clippy::too_many_arguments)]
fn text(
    pm: &mut Pixmap,
    font: &FontVec,
    size: f32,
    s: &str,
    cx: f32,
    baseline: f32,
    rgb: [u8; 3],
    alpha: f32,
) {
    let scaled = font.as_scaled(PxScale::from(size));
    let mut width = 0.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            width += scaled.kern(p, id);
        }
        width += scaled.h_advance(id);
        prev = Some(id);
    }
    let (pw, ph) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let mut x = cx - width / 2.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(p) = prev {
            x += scaled.kern(p, id);
        }
        let glyph = id.with_scale_and_position(size, ab_glyph::point(x, baseline));
        x += scaled.h_advance(id);
        prev = Some(id);
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let b = outline.px_bounds();
        outline.draw(|gx, gy, cov| {
            let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
            if px < 0 || py < 0 || px >= pw || py >= ph {
                return;
            }
            let a = (cov.clamp(0.0, 1.0) * alpha).clamp(0.0, 1.0);
            let i = ((py * pw + px) * 4) as usize;
            // premultiplied "over"
            for (c, v) in rgb.iter().enumerate() {
                let src = *v as f32 * a;
                data[i + c] = (src + data[i + c] as f32 * (1.0 - a)).round() as u8;
            }
            data[i + 3] = (a * 255.0 + data[i + 3] as f32 * (1.0 - a)).round() as u8;
        });
    }
}

/// The font file fontconfig picks for `family` (or sans-serif).
fn load_font(family: Option<&str>) -> Option<FontVec> {
    let pattern = family.map_or_else(
        || "sans-serif".to_string(),
        |f| format!("{f}:style=Regular"),
    );
    for pat in [pattern.as_str(), "sans-serif"] {
        let out = std::process::Command::new("fc-match")
            .args(["-f", "%{file}\n%{index}", pat])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let mut lines = text.lines();
        let (Some(file), index) = (lines.next(), lines.next()) else {
            continue;
        };
        let index = index.and_then(|i| i.trim().parse().ok()).unwrap_or(0);
        if let Ok(bytes) = std::fs::read(file.trim())
            && let Ok(f) = FontVec::try_from_vec_and_index(bytes, index)
        {
            return Some(f);
        }
    }
    None
}

/// The local time as (`HH:MM`, a localized date like "Sunday, 22 November").
pub fn now_strings() -> (String, String) {
    // SAFETY: time/localtime_r/strftime into our own buffers.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let fmt = |f: &std::ffi::CStr| {
            let mut buf = [0u8; 128];
            let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), f.as_ptr(), &tm);
            String::from_utf8_lossy(&buf[..n]).into_owned()
        };
        (fmt(c"%H:%M"), fmt(c"%A, %d %B"))
    }
}

/// Seconds until the next minute starts (when the clock changes).
pub fn secs_to_next_minute() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    60 - now % 60
}

/// Copies tiny-skia's premultiplied RGBA into wl_shm ARGB8888 (B, G, R, A
/// in memory).
pub fn rgba_to_argb(src: &[u8], dst: &mut [u8]) {
    for (s, d) in src
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dst.as_chunks_mut::<4>().0)
    {
        d[0] = s[2];
        d[1] = s[1];
        d[2] = s[0];
        d[3] = s[3];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_draws_every_state() {
        let panel = Panel::new(Theme::default());
        for (status, typed, caps) in [
            (Status::Idle, 0, false),
            (Status::Idle, 5, true),
            (Status::Checking, 5, false),
            (Status::Failed("Authentication failure".into()), 0, false),
        ] {
            let pm = panel
                .draw(
                    &PanelState {
                        time: "12:34",
                        date: "Sunday, 22 November",
                        typed,
                        status: &status,
                        caps_lock: caps,
                    },
                    1.5,
                )
                .unwrap();
            assert_eq!((pm.width(), pm.height()), Panel::pixel_size(1.5));
            assert!(
                pm.data().as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "something drawn"
            );
        }
    }

    #[test]
    fn argb_swaps_red_and_blue() {
        let mut out = [0u8; 4];
        rgba_to_argb(&[1, 2, 3, 4], &mut out);
        assert_eq!(out, [3, 2, 1, 4]);
    }
}
