// Minimal Hyprland IPC client (request socket only).
//
// The engine needs the compositor for more than a wallpaper usually does:
// it creates headless outputs where WE's windows render out of sight, pins
// workspaces and window rules to them, and asks which real monitors are
// covered by a fullscreen window (pause there). The Lua dispatchers used here
// (`hl.dsp.*`, `hl.window_rule`, ...) need Hyprland 0.56+.

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

pub fn json(cmd: &str) -> Option<serde_json::Value> {
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

/// One output as Hyprland sees it.
#[derive(Debug, Clone)]
pub struct Monitor {
    pub name: String,
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
                let ws = m.pointer("/activeWorkspace/id").and_then(|i| i.as_i64());
                Some(Monitor {
                    name: m.get("name")?.as_str()?.to_string(),
                    covered: ws.is_some_and(|w| fullscreen_ws.contains(&w)),
                })
            })
            .collect(),
    )
}

/// Runs a request that answers "ok" on success (`dispatch ...`, `eval ...`,
/// `output ...`).
pub fn command(cmd: &str) -> bool {
    request(cmd).is_some_and(|reply| reply.trim() == "ok")
}

/// `hyprctl dispatch <lua>`, e.g. `hl.dsp.focus({ ... })`.
pub fn dispatch(lua: &str) -> bool {
    command(&format!("dispatch {lua}"))
}

/// `hyprctl eval <lua>`, e.g. `hl.window_rule({ ... })`.
pub fn eval(lua: &str) -> bool {
    command(&format!("eval {lua}"))
}

/// Every window, as `hyprctl clients -j` lists them.
pub fn clients() -> Vec<serde_json::Value> {
    json("j/clients")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Every output, as `hyprctl monitors -j` lists them.
pub fn monitors_raw() -> Vec<serde_json::Value> {
    json("j/monitors")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Address of the focused window, if any.
pub fn active_window() -> Option<String> {
    json("j/activewindow")?
        .get("address")?
        .as_str()
        .map(String::from)
        .filter(|a| !a.is_empty())
}

pub fn create_headless(name: &str) -> bool {
    command(&format!("output create headless {name}"))
}
