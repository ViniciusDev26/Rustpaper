//! Hyprland adapter (0.56+: Lua dispatchers `hl.dsp.*`, `hl.window_rule`, ...).
//!
//! - **Desktop.** Monitors and what covers them come from the IPC socket
//!   (`j/monitors`, `j/clients`, `j/activewindow`).
//! - **Stage.** Each real monitor gets a headless output `RP-<output>` of its
//!   pixel size whose default workspace `rustpaper-<output>` is pinned to
//!   it. A window rule sends WE's window there fullscreen, without focus or
//!   animation. `tend` repairs what other programs disturb (workspace
//!   switched away, window out of fullscreen).

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{Coverage, Desktop, Output, Stage, WE_CLASS, window_title};

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
fn disable_layer_animations() -> bool {
    const RULES: [&str; 3] = [
        r#"eval hl.layer_rule({ match = { namespace = "^rustpaper$" }, no_anim = true, animation = "none" })"#,
        "keyword layerrule no_anim on, match:namespace ^rustpaper$",
        "keyword layerrule noanim, ^rustpaper$",
    ];
    RULES
        .iter()
        .any(|r| request(r).is_some_and(|reply| reply.trim() == "ok"))
}

fn monitors() -> Option<Vec<Coverage>> {
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
) -> Vec<Coverage> {
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
                return Some(Coverage {
                    name,
                    ..Default::default()
                });
            };
            Some(Coverage {
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
fn command(cmd: &str) -> bool {
    request(cmd).is_some_and(|reply| reply.trim() == "ok")
}

/// `hyprctl dispatch <lua>`, e.g. `hl.dsp.focus({ ... })`.
fn dispatch(lua: &str) -> bool {
    command(&format!("dispatch {lua}"))
}

/// `hyprctl eval <lua>`, e.g. `hl.window_rule({ ... })`.
fn eval(lua: &str) -> bool {
    command(&format!("eval {lua}"))
}

/// Every window, as `hyprctl clients -j` lists them.
fn clients() -> Vec<serde_json::Value> {
    json("j/clients")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Every output, as `hyprctl monitors -j` lists them.
fn monitors_raw() -> Vec<serde_json::Value> {
    json("j/monitors")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Address of the focused window, if any.
fn active_window() -> Option<String> {
    json("j/activewindow")?
        .get("address")?
        .as_str()
        .map(String::from)
        .filter(|a| !a.is_empty())
}

fn create_headless(name: &str) -> bool {
    command(&format!("output create headless {name}"))
}

/// Hidden workspace for WE's own windows (its UI, dialogs).
const WE_UI_WORKSPACE: &str = "special:rustpaper-we";

/// Outputs that are not real monitors: ours, the old harness/spike ones,
/// Hyprland's default headless names.
fn is_headless(name: &str) -> bool {
    ["RP-", "RPREF", "RPWRAP", "HEADLESS"]
        .iter()
        .any(|p| name.starts_with(p))
}

/// The headless output hosting monitor `output`'s WE window.
fn headless_name(output: &str) -> String {
    format!("RP-{output}")
}

pub struct Hyprland {
    /// Set by the event listener when Hyprland reloaded its config.
    reloaded: Arc<AtomicBool>,
}

impl Hyprland {
    pub fn connect() -> Result<Self, String> {
        json("j/version").ok_or(
            "Hyprland's IPC socket does not answer (Rustpaper needs Hyprland 0.56 or newer)",
        )?;
        let reloaded = Arc::new(AtomicBool::new(false));
        listen_for_reloads(reloaded.clone());
        Ok(Hyprland { reloaded })
    }
}

/// A config reload drops every rule added at runtime: our window rules (no
/// focus, no animation, compositor-only fullscreen), workspace pins and the
/// headless outputs' modes. Omarchy reloads on every theme switch, which the
/// selector's "Theme from wallpaper" triggers. Seen 2026-11-22: after one,
/// WE's windows animated from 7680x4320 down to their size (the capture
/// resized every frame). Hyprland announces reloads on its event socket
/// (`.socket2.sock`, line `configreloaded>>`): the Stage is then re-applied.
fn listen_for_reloads(flag: Arc<AtomicBool>) {
    use std::io::BufRead;
    let Some(path) = socket_path().map(|p| p.with_file_name(".socket2.sock")) else {
        return;
    };
    std::thread::spawn(move || {
        loop {
            if let Ok(stream) = UnixStream::connect(&path) {
                for line in std::io::BufReader::new(stream).lines() {
                    let Ok(line) = line else { break };
                    if line.starts_with("configreloaded>>") {
                        flag.store(true, Ordering::SeqCst);
                    }
                }
            }
            // Hyprland restarted or the socket hiccuped: try again
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

impl Desktop for Hyprland {
    fn name(&self) -> &'static str {
        "Hyprland"
    }

    fn outputs(&self) -> Vec<Output> {
        monitors_raw()
            .iter()
            .filter_map(|m| {
                let name = m.get("name")?.as_str()?.to_string();
                let w = m.get("width")?.as_u64()? as u32;
                let h = m.get("height")?.as_u64()? as u32;
                (!is_headless(&name)).then_some(Output { name, size: (w, h) })
            })
            .collect()
    }

    fn coverage(&self) -> Option<Vec<Coverage>> {
        monitors()
    }

    fn prepare_layers(&self) {
        disable_layer_animations();
    }

    fn stage(&self) -> Option<&dyn Stage> {
        Some(self)
    }
}

impl Stage for Hyprland {
    fn prepare(&self, out: &Output) {
        let (w, h) = out.size;
        let (ws, headless) = (window_title(&out.name), headless_name(&out.name));
        // the rule first: the new output then opens straight on our workspace
        eval(&format!(
            "hl.workspace_rule({{ workspace = 'name:{ws}', monitor = '{headless}', default = true, persistent = true }})"
        ));
        let exists = monitors_raw()
            .iter()
            .any(|m| m.get("name").and_then(|n| n.as_str()) == Some(&headless));
        if !exists {
            create_headless(&headless);
        }
        eval(&format!(
            "hl.monitor({{ output = '{headless}', mode = '{w}x{h}@60', position = 'auto', scale = 1 }})"
        ));
        // When the idle daemon powers the monitors off (DPMS) a headless
        // output stops rendering too: keep ours on.
        dispatch(&format!(
            "hl.dsp.dpms({{ action = 'on', monitor = '{headless}' }})"
        ));
        // WE's window: straight onto that workspace, fullscreen, never
        // focused, no animation (the fullscreen grow resized the capture
        // every frame). Fullscreen for the compositor only (`fullscreen_state`
        // internal 2, client 0): told it was fullscreen, the window (Wine)
        // kept leaving it, tiled at 1896x1026 every couple of seconds; not
        // told, it stays.
        eval(&format!(
            "hl.window_rule({{ name = '{ws}', match = {{ class = '{WE_CLASS}', title = '^{ws}$' }}, workspace = 'name:{ws} silent', fullscreen_state = '2 0', no_anim = true, no_focus = true, no_initial_focus = true, focus_on_activate = false }})"
        ));
    }

    fn hide_we_ui(&self) {
        eval(&format!(
            "hl.window_rule({{ name = 'rustpaper-we', match = {{ class = '{WE_CLASS}' }}, no_focus = true, no_initial_focus = true, focus_on_activate = false, no_anim = true }})"
        ));
        for c in clients() {
            let ours = c
                .get("title")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t.starts_with("rustpaper-"));
            let we = c.get("class").and_then(|t| t.as_str()) == Some(WE_CLASS);
            let hidden =
                c.pointer("/workspace/name").and_then(|n| n.as_str()) == Some(WE_UI_WORKSPACE);
            if we
                && !ours
                && !hidden
                && let Some(a) = c.get("address").and_then(|a| a.as_str())
            {
                dispatch(&format!(
                    "hl.dsp.window.move({{ window = 'address:{a}', workspace = '{WE_UI_WORKSPACE}', follow = false }})"
                ));
            }
        }
    }

    fn tend(&self, out: &Output) -> bool {
        if self.reloaded.swap(false, Ordering::SeqCst) {
            eprintln!("hyprland: config reloaded, restoring the stage");
            self.hide_we_ui();
            // every output (the flag is shared and cleared by the first)
            for o in self.outputs() {
                self.prepare(&o);
            }
        }
        let (ws, headless) = (window_title(&out.name), headless_name(&out.name));
        let clients = clients();
        let Some(c) = clients
            .iter()
            .find(|c| c.get("title").and_then(|x| x.as_str()) == Some(&ws))
        else {
            return false;
        };
        let addr = c
            .get("address")
            .and_then(|a| a.as_str())
            .unwrap_or_default();
        if c.pointer("/workspace/name").and_then(|n| n.as_str()) != Some(&ws) {
            dispatch(&format!(
                "hl.dsp.window.move({{ window = 'address:{addr}', workspace = 'name:{ws}', follow = false }})"
            ));
        }
        // keep it fullscreen (compositor-side only, see prepare)
        let fs = (
            c.get("fullscreen").and_then(|f| f.as_i64()),
            c.get("fullscreenClient").and_then(|f| f.as_i64()),
        );
        if fs != (Some(2), Some(0)) {
            dispatch(&format!(
                "hl.dsp.window.fullscreen_state({{ internal = 2, client = 0, action = 'set', window = 'address:{addr}' }})"
            ));
        }
        // a window on a hidden workspace is not rendered: the capture stalls
        // (seen when a new output was created: ours briefly showed others)
        let shown = monitors_raw()
            .iter()
            .find(|m| m.get("name").and_then(|n| n.as_str()) == Some(&headless))
            .and_then(|m| {
                m.pointer("/activeWorkspace/name")?
                    .as_str()
                    .map(String::from)
            });
        if shown.as_deref().is_some_and(|s| s != ws) {
            let me = active_window();
            dispatch(&format!("hl.dsp.focus({{ workspace = 'name:{ws}' }})"));
            if let Some(me) = me {
                dispatch(&format!("hl.dsp.focus({{ window = 'address:{me}' }})"));
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn headless_outputs_are_not_monitors() {
        assert!(is_headless("RP-DP-1"));
        assert!(is_headless("RPREF"));
        assert!(is_headless("HEADLESS-2"));
        assert!(!is_headless("DP-1"));
        assert!(!is_headless("HDMI-A-1"));
    }

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
