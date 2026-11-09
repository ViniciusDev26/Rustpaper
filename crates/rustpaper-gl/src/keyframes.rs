// Keyframed property animations (the editor's timeline). A property can be
//
//   "alpha": { "value": 1.0, "animation": {
//       "options": { "fps": 2, "length": 1, "mode": "mirror" },
//       "c0": [ { "frame": 0, "value": 0, "back": {...}, "front": {...} }, ... ],
//       "c1": [...], "c2": [...] } }        // one curve per component
//
// Keys are interpolated linearly, or along a cubic Bezier when the outgoing
// `front` / incoming `back` handle is enabled (handle x in frames, y in value
// units, relative to the key). `length` is in frames; `mode` is loop,
// mirror (ping-pong) or single (hold the last key).

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Handle {
    enabled: bool,
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Key {
    frame: f32,
    value: f32,
    back: Handle,
    front: Handle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Loop,
    Mirror,
    Single,
}

#[derive(Clone, Debug)]
pub struct Track {
    curves: Vec<Vec<Key>>,
    fps: f32,
    length: f32,
    mode: Mode,
}

fn num(v: Option<&Value>, d: f32) -> f32 {
    v.and_then(|x| x.as_f64()).map(|x| x as f32).unwrap_or(d)
}

fn handle(v: Option<&Value>, x: f32) -> Handle {
    Handle {
        enabled: v
            .and_then(|h| h.get("enabled"))
            .and_then(|e| e.as_bool())
            .unwrap_or(false),
        x: num(v.and_then(|h| h.get("x")), x),
        y: num(v.and_then(|h| h.get("y")), 0.0),
    }
}

impl Track {
    /// The animation of a property value, if it has one.
    pub fn parse(property: &Value) -> Option<Track> {
        let a = property.get("animation")?;
        let opts = a.get("options");
        let mut curves = Vec::new();
        for c in 0.. {
            let Some(keys) = a.get(format!("c{c}")).and_then(|k| k.as_array()) else {
                break;
            };
            let mut keys: Vec<Key> = keys
                .iter()
                .map(|k| Key {
                    frame: num(k.get("frame"), 0.0),
                    value: num(k.get("value"), 0.0),
                    back: handle(k.get("back"), -1.0),
                    front: handle(k.get("front"), 1.0),
                })
                .collect();
            keys.sort_by(|a, b| a.frame.total_cmp(&b.frame));
            curves.push(keys);
        }
        if curves.iter().all(|c| c.is_empty()) {
            return None;
        }
        Some(Track {
            curves,
            fps: num(opts.and_then(|o| o.get("fps")), 30.0).max(1e-3),
            length: num(opts.and_then(|o| o.get("length")), 0.0),
            mode: match opts.and_then(|o| o.get("mode")).and_then(|m| m.as_str()) {
                Some("mirror") => Mode::Mirror,
                Some("single") => Mode::Single,
                _ => Mode::Loop,
            },
        })
    }

    /// Component values at `time` seconds.
    pub fn eval(&self, time: f32) -> Vec<f32> {
        let last = self
            .curves
            .iter()
            .filter_map(|c| c.last())
            .map(|k| k.frame)
            .fold(0.0, f32::max);
        let len = if self.length > 0.0 { self.length } else { last };
        let mut f = (time * self.fps).max(0.0);
        if len > 0.0 {
            f = match self.mode {
                Mode::Loop => f % len,
                Mode::Mirror => {
                    let c = f % (2.0 * len);
                    if c > len { 2.0 * len - c } else { c }
                }
                Mode::Single => f.min(len),
            };
        }
        self.curves.iter().map(|c| curve_at(c, f)).collect()
    }
}

fn curve_at(keys: &[Key], f: f32) -> f32 {
    let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
        return 0.0;
    };
    if f <= first.frame {
        return first.value;
    }
    if f >= last.frame {
        return last.value;
    }
    let i = keys.iter().rposition(|k| k.frame <= f).unwrap_or(0);
    let (a, b) = (keys[i], keys[(i + 1).min(keys.len() - 1)]);
    let span = b.frame - a.frame;
    if span <= 0.0 {
        return b.value;
    }
    if !a.front.enabled && !b.back.enabled {
        return a.value + (b.value - a.value) * (f - a.frame) / span;
    }
    // Bezier with handle x clamped inside the segment (keeps x(t) monotonic)
    let (fx, fy) = if a.front.enabled {
        (a.front.x.clamp(0.0, span), a.front.y)
    } else {
        (span / 3.0, (b.value - a.value) / 3.0)
    };
    let (bx, by) = if b.back.enabled {
        (b.back.x.clamp(-span, 0.0), b.back.y)
    } else {
        (-span / 3.0, -(b.value - a.value) / 3.0)
    };
    let (x0, x1, x2, x3) = (a.frame, a.frame + fx, b.frame + bx, b.frame);
    let (y0, y1, y2, y3) = (a.value, a.value + fy, b.value + by, b.value);
    let bez = |p0: f32, p1: f32, p2: f32, p3: f32, t: f32| {
        let u = 1.0 - t;
        u * u * u * p0 + 3.0 * u * u * t * p1 + 3.0 * u * t * t * p2 + t * t * t * p3
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if bez(x0, x1, x2, x3, mid) < f {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(y0, y1, y2, y3, (lo + hi) / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(frame: f32, value: f32) -> Value {
        json!({ "frame": frame, "value": value,
                "back": { "enabled": false, "x": -1, "y": 0 },
                "front": { "enabled": false, "x": 1, "y": 0 } })
    }

    #[test]
    fn linear_mirror_blinks() {
        // Red Light Left of "Summer Feeling": 0 -> 1 over 1 frame at 2 fps, mirrored
        let p = json!({ "value": 1.0, "animation": {
            "options": { "fps": 2, "length": 1, "mode": "mirror" },
            "c0": [key(0.0, 0.0), key(1.0, 1.0)] } });
        let t = Track::parse(&p).unwrap();
        assert_eq!(t.eval(0.0), vec![0.0]);
        assert!((t.eval(0.25)[0] - 0.5).abs() < 1e-4);
        assert!((t.eval(0.5)[0] - 1.0).abs() < 1e-4);
        assert!((t.eval(0.75)[0] - 0.5).abs() < 1e-4, "comes back");
    }

    #[test]
    fn eased_keys_stay_between_values_and_loop() {
        let mut a = key(0.0, 0.0);
        a["front"]["enabled"] = json!(true);
        let mut b = key(10.0, 1.0);
        b["back"]["enabled"] = json!(true);
        let p = json!({ "animation": { "options": { "fps": 10, "length": 20, "mode": "loop" }, "c0": [a, b] } });
        let t = Track::parse(&p).unwrap();
        let mid = t.eval(0.5)[0];
        assert!((mid - 0.5).abs() < 0.05, "{mid}");
        assert!(t.eval(0.1)[0] < 0.1, "ease-in starts slowly");
        assert!(
            (t.eval(1.5)[0] - 1.0).abs() < 1e-4,
            "holds after the last key"
        );
        assert!(t.eval(2.0)[0].abs() < 1e-4, "loops");
        assert!(Track::parse(&json!({ "value": 1 })).is_none());
    }
}
