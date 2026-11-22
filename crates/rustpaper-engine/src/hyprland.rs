// Minimal Hyprland IPC client (request socket only).
//
// The wallpaper lives on a layer surface that never receives pointer input, so
// the compositor is asked directly: where the cursor is (cursor-reactive
// effects, parallax) and which outputs are covered by a fullscreen window
// (pause rendering there). On other compositors every call returns None and
// the engine falls back to "cursor at the center, never paused".

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

fn socket_path() -> Option<PathBuf> {
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let p = PathBuf::from(runtime)
        .join("hypr")
        .join(sig)
        .join(".socket.sock");
    p.exists().then_some(p)
}

/// Sends one request (e.g. "j/cursorpos") and returns the raw reply.
fn request(cmd: &str) -> Option<String> {
    let mut s = UnixStream::connect(socket_path()?).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(200))).ok()?;
    s.write_all(cmd.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

fn json(cmd: &str) -> Option<serde_json::Value> {
    serde_json::from_str(&request(cmd)?).ok()
}

/// Turns off Hyprland's layer animations for our surfaces (namespace
/// `rustpaper`), before any is created. Switching wallpapers starts the new
/// engine and kills the old one 2.5 s later; with layersOut/fadeLayersOut
/// on, Hyprland faded out a snapshot of the old surface and the previous
/// wallpaper flashed over the new one (reported on Hyprland 0.56; Omarchy
/// sets the same rule on its own background layers for that reason).
///
/// The rule syntax changed across Hyprland versions, so each one is tried
/// until the compositor answers "ok": Lua config (`eval`), hyprlang rules
/// v2 (0.53+), then the legacy `noanim`. Every engine start adds the rule
/// again; duplicates are harmless and the set is dropped on config reload.
pub fn disable_layer_animations() -> bool {
    const RULES: [&str; 3] = [
        r#"eval hl.layer_rule({ match = { namespace = "^rustpaper$" }, no_anim = true, animation = "none" })"#,
        "keyword layerrule no_anim on, match:namespace ^rustpaper$",
        "keyword layerrule noanim, ^rustpaper$",
    ];
    RULES
        .iter()
        .any(|r| request(r).is_some_and(|reply| reply.trim() == "ok"))
}

/// Cursor in global logical coordinates.
pub fn cursor() -> Option<(f64, f64)> {
    let v = json("j/cursorpos")?;
    Some((v.get("x")?.as_f64()?, v.get("y")?.as_f64()?))
}

/// One output as Hyprland sees it.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
    /// logical rectangle (x, y, w, h)
    pub rect: (f64, f64, f64, f64),
    /// a fullscreen window is shown on its active workspace
    pub covered: bool,
}

pub fn monitors() -> Option<Vec<Monitor>> {
    let mons = json("j/monitors")?;
    let clients = json("j/clients").unwrap_or_default();
    let fullscreen_ws: Vec<i64> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c.get("fullscreen").and_then(|f| f.as_i64()).unwrap_or(0) > 0)
        .filter(|c| c.get("hidden").and_then(|h| h.as_bool()) != Some(true))
        .filter_map(|c| c.pointer("/workspace/id").and_then(|i| i.as_i64()))
        .collect();
    Some(
        mons.as_array()?
            .iter()
            .filter_map(|m| {
                let scale = m.get("scale")?.as_f64()?.max(0.1);
                let (w, h) = (m.get("width")?.as_f64()?, m.get("height")?.as_f64()?);
                // `transform` 1/3/5/7 rotate by 90 degrees: swap the axes
                let rotated = m.get("transform").and_then(|t| t.as_i64()).unwrap_or(0) % 2 == 1;
                let (lw, lh) = if rotated {
                    (h / scale, w / scale)
                } else {
                    (w / scale, h / scale)
                };
                let ws = m.pointer("/activeWorkspace/id").and_then(|i| i.as_i64());
                Some(Monitor {
                    name: m.get("name")?.as_str()?.to_string(),
                    rect: (m.get("x")?.as_f64()?, m.get("y")?.as_f64()?, lw, lh),
                    covered: ws.is_some_and(|w| fullscreen_ws.contains(&w)),
                })
            })
            .collect(),
    )
}
