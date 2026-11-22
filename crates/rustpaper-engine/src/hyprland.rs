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

/// What covers one output (the conditions of the Playback settings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Monitor {
    pub name: String,
    /// A fullscreen window is on its active workspace.
    pub fullscreen: bool,
    /// A maximized or tiled window is on its active workspace (Hyprland
    /// tiles by default: the wallpaper then only shows in the gaps).
    pub maximized: bool,
    /// The focused window is on its active workspace.
    pub focused: bool,
    /// Powered off (DPMS).
    pub asleep: bool,
}

pub fn monitors() -> Option<Vec<Monitor>> {
    let mons = json("j/monitors")?;
    let clients = json("j/clients").unwrap_or_default();
    let focused_ws = json("j/activewindow").and_then(|w| w.pointer("/workspace/id")?.as_i64());
    Some(covering(&mons, &clients, focused_ws))
}

/// `monitors()` from `hyprctl monitors -j` / `clients -j` replies (pure, for
/// tests).
fn covering(
    mons: &serde_json::Value,
    clients: &serde_json::Value,
    focused_ws: Option<i64>,
) -> Vec<Monitor> {
    let visible: Vec<&serde_json::Value> = clients
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c.get("hidden").and_then(|h| h.as_bool()) != Some(true))
        .filter(|c| c.get("mapped").and_then(|h| h.as_bool()) != Some(false))
        .collect();
    let on = |ws: i64, pred: &dyn Fn(&serde_json::Value) -> bool| {
        visible
            .iter()
            .any(|c| c.pointer("/workspace/id").and_then(|i| i.as_i64()) == Some(ws) && pred(c))
    };
    // `fullscreen`: 0 none, 1 maximized, 2 fullscreen
    let mode = |c: &serde_json::Value| c.get("fullscreen").and_then(|f| f.as_i64()).unwrap_or(0);
    let floating =
        |c: &serde_json::Value| c.get("floating").and_then(|f| f.as_bool()).unwrap_or(false);
    mons.as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let ws = m.pointer("/activeWorkspace/id").and_then(|i| i.as_i64());
            let name = m.get("name")?.as_str()?.to_string();
            let Some(ws) = ws else {
                return Some(Monitor {
                    name,
                    ..Default::default()
                });
            };
            Some(Monitor {
                name,
                fullscreen: on(ws, &|c| mode(c) >= 2),
                maximized: on(ws, &|c| mode(c) == 1 || (mode(c) == 0 && !floating(c))),
                focused: focused_ws == Some(ws),
                asleep: m.get("dpmsStatus").and_then(|d| d.as_bool()) == Some(false),
            })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn what_covers_each_monitor() {
        let mons = json!([
            { "name": "A", "activeWorkspace": { "id": 1 }, "dpmsStatus": true },
            { "name": "B", "activeWorkspace": { "id": 2 }, "dpmsStatus": true },
            { "name": "C", "activeWorkspace": { "id": 3 }, "dpmsStatus": false },
            { "name": "D", "activeWorkspace": { "id": 4 }, "dpmsStatus": true },
        ]);
        let clients = json!([
            { "workspace": { "id": 1 }, "fullscreen": 2, "floating": false },
            { "workspace": { "id": 2 }, "fullscreen": 0, "floating": false },
            { "workspace": { "id": 4 }, "fullscreen": 0, "floating": true },
            { "workspace": { "id": 4 }, "fullscreen": 0, "floating": false, "hidden": true },
        ]);
        let m = covering(&mons, &clients, Some(2));
        assert!(m[0].fullscreen && !m[0].focused);
        assert!(
            m[1].maximized && !m[1].fullscreen && m[1].focused,
            "a tiled window"
        );
        assert!(m[2].asleep && !m[2].maximized);
        assert!(
            !m[3].maximized,
            "floating and hidden windows leave the wallpaper visible"
        );
    }
}
