//! Sway adapter (Sway 1.11+: wlroots 0.19 brings `ext-image-copy-capture`
//! and `ext-foreign-toplevel-list`, which the mirror needs).
//!
//! Talks to Sway's IPC socket (`$SWAYSOCK`, the i3 IPC protocol).
//! - **Desktop.** Monitors come from `GET_OUTPUTS`, coverage from
//!   `GET_TREE`: the visible workspace's containers.
//! - **Stage.** Each real monitor gets a headless output (`create_output`,
//!   named `HEADLESS-<n>` by Sway, so the mapping is the workspace
//!   `rustpaper-<output>` assigned to it). `for_window` rules send WE's
//!   window there fullscreen and keep it from taking focus; WE's other
//!   windows go to the scratchpad.
//!
//! Status: written against Sway's IPC documentation (sway-ipc(7)) with unit
//! tests on recorded replies; not run on a live Sway session yet.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use serde_json::Value;

use super::{Coverage, Desktop, Output, Stage, WE_CLASS, window_title};

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_WORKSPACES: u32 = 1;
const GET_OUTPUTS: u32 = 3;
const GET_TREE: u32 = 4;

/// One IPC message: magic, payload length, type (native endian), payload.
fn encode(kind: u32, payload: &str) -> Vec<u8> {
    let mut m = MAGIC.to_vec();
    m.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
    m.extend_from_slice(&kind.to_ne_bytes());
    m.extend_from_slice(payload.as_bytes());
    m
}

fn request(kind: u32, payload: &str) -> Option<Value> {
    let path = std::env::var_os("SWAYSOCK")?;
    let mut s = UnixStream::connect(path).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(&encode(kind, payload)).ok()?;
    let mut header = [0u8; 14];
    s.read_exact(&mut header).ok()?;
    if &header[..6] != MAGIC {
        return None;
    }
    let len = u32::from_ne_bytes(header[6..10].try_into().ok()?) as usize;
    let mut body = vec![0u8; len];
    s.read_exact(&mut body).ok()?;
    serde_json::from_slice(&body).ok()
}

/// Runs sway commands; true when all succeeded.
fn command(cmd: &str) -> bool {
    request(RUN_COMMAND, cmd)
        .and_then(|v| v.as_array().cloned())
        .is_some_and(|r| {
            r.iter()
                .all(|x| x.get("success").and_then(|s| s.as_bool()) == Some(true))
        })
}

fn outputs_raw() -> Vec<Value> {
    request(GET_OUTPUTS, "")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

/// Outputs Sway created headless (ours, or the user's own).
fn is_headless(name: &str) -> bool {
    name.starts_with("HEADLESS-")
}

/// Every container of the tree, depth first.
fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Value>) {
    out.push(node);
    for key in ["nodes", "floating_nodes"] {
        for child in node
            .get(key)
            .and_then(|n| n.as_array())
            .into_iter()
            .flatten()
        {
            walk(child, out);
        }
    }
}

/// The window container titled `title` (`name` in sway's tree).
fn find_window<'a>(tree: &'a Value, title: &str) -> Option<&'a Value> {
    let mut all = Vec::new();
    walk(tree, &mut all);
    all.into_iter()
        .find(|n| n.get("name").and_then(|t| t.as_str()) == Some(title) && n.get("pid").is_some())
}

/// Coverage of each real output, from `GET_OUTPUTS` + `GET_TREE` replies.
fn covering(outputs: &[Value], tree: &Value) -> Vec<Coverage> {
    let mut all = Vec::new();
    walk(tree, &mut all);
    let workspace = |name: &str| {
        all.iter()
            .find(|n| {
                n.get("type").and_then(|t| t.as_str()) == Some("workspace")
                    && n.get("name").and_then(|t| t.as_str()) == Some(name)
            })
            .copied()
    };
    outputs
        .iter()
        .filter_map(|o| {
            let name = o.get("name")?.as_str()?.to_string();
            if is_headless(&name) || o.get("active").and_then(|a| a.as_bool()) == Some(false) {
                return None;
            }
            let asleep = o
                .get("power")
                .or_else(|| o.get("dpms"))
                .and_then(|p| p.as_bool())
                == Some(false);
            let mut c = Coverage {
                name,
                asleep,
                ..Default::default()
            };
            let ws = o
                .get("current_workspace")
                .and_then(|w| w.as_str())
                .and_then(workspace);
            if let Some(ws) = ws {
                let mut inside = Vec::new();
                walk(ws, &mut inside);
                let windows: Vec<&&Value> =
                    inside.iter().filter(|n| n.get("pid").is_some()).collect();
                c.fullscreen = windows.iter().any(|n| {
                    n.get("fullscreen_mode")
                        .and_then(|f| f.as_i64())
                        .unwrap_or(0)
                        > 0
                });
                // tiled windows live under `nodes`; floating ones have type floating_con
                c.maximized = windows
                    .iter()
                    .any(|n| n.get("type").and_then(|t| t.as_str()) == Some("con"));
                c.focused = inside
                    .iter()
                    .any(|n| n.get("focused").and_then(|f| f.as_bool()) == Some(true));
            }
            Some(c)
        })
        .collect()
}

pub struct Sway;

impl Sway {
    pub fn connect() -> Result<Self, String> {
        request(GET_OUTPUTS, "")
            .map(|_| Sway)
            .ok_or_else(|| "Sway's IPC socket ($SWAYSOCK) does not answer".into())
    }
}

impl Desktop for Sway {
    fn name(&self) -> &'static str {
        "Sway"
    }

    fn outputs(&self) -> Vec<Output> {
        outputs_raw()
            .iter()
            .filter(|o| o.get("active").and_then(|a| a.as_bool()) != Some(false))
            .filter_map(|o| {
                let name = o.get("name")?.as_str()?.to_string();
                let mode = o.get("current_mode")?;
                let size = (
                    mode.get("width")?.as_u64()? as u32,
                    mode.get("height")?.as_u64()? as u32,
                );
                (!is_headless(&name)).then_some(Output { name, size })
            })
            .collect()
    }

    fn coverage(&self) -> Option<Vec<Coverage>> {
        let tree = request(GET_TREE, "")?;
        Some(covering(&outputs_raw(), &tree))
    }

    fn stage(&self) -> Option<&dyn Stage> {
        Some(self)
    }
}

impl Stage for Sway {
    fn prepare(&self, out: &Output) {
        let ws = window_title(&out.name);
        let (w, h) = out.size;
        // reuse the headless output our workspace already lives on
        let assigned = request(GET_WORKSPACES, "")
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default()
            .into_iter()
            .find(|x| x.get("name").and_then(|n| n.as_str()) == Some(&ws))
            .and_then(|x| x.get("output")?.as_str().map(String::from))
            .filter(|o| is_headless(o));
        let headless = assigned.or_else(|| {
            let before: Vec<String> = outputs_raw()
                .iter()
                .filter_map(|o| o.get("name")?.as_str().map(String::from))
                .collect();
            command("create_output");
            outputs_raw()
                .iter()
                .filter_map(|o| o.get("name")?.as_str().map(String::from))
                .find(|n| is_headless(n) && !before.contains(n))
        });
        let Some(headless) = headless else {
            eprintln!("sway: could not create a headless output for {}", out.name);
            return;
        };
        command(&format!("output {headless} mode --custom {w}x{h}@60Hz"));
        command(&format!("workspace {ws} output {headless}"));
        command(&format!(
            "for_window [class=\"{WE_CLASS}\" title=\"^{ws}$\"] move container to workspace {ws}, fullscreen enable"
        ));
        command(&format!("no_focus [class=\"{WE_CLASS}\" title=\"^{ws}$\"]"));
    }

    fn hide_we_ui(&self) {
        command(&format!("no_focus [class=\"{WE_CLASS}\"]"));
        // sway's criteria are PCRE2: everything of WE's but our windows
        command(&format!(
            "for_window [class=\"{WE_CLASS}\" title=\"^(?!rustpaper-)\"] move scratchpad"
        ));
    }

    fn tend(&self, out: &Output) -> bool {
        let ws = window_title(&out.name);
        let Some(tree) = request(GET_TREE, "") else {
            return false;
        };
        let Some(win) = find_window(&tree, &ws) else {
            return false;
        };
        let Some(id) = win.get("id").and_then(|i| i.as_i64()) else {
            return true;
        };
        if win
            .get("fullscreen_mode")
            .and_then(|f| f.as_i64())
            .unwrap_or(0)
            == 0
        {
            command(&format!(
                "[con_id={id}] move container to workspace {ws}, fullscreen enable"
            ));
        }
        // the workspace must be the one shown on its headless output
        let shown = outputs_raw()
            .iter()
            .any(|o| o.get("current_workspace").and_then(|w| w.as_str()) == Some(&ws));
        if !shown {
            let mut all = Vec::new();
            walk(&tree, &mut all);
            let focused = all
                .iter()
                .find(|n| n.get("focused").and_then(|f| f.as_bool()) == Some(true))
                .and_then(|n| n.get("id")?.as_i64());
            command(&format!("workspace {ws}"));
            if let Some(f) = focused {
                command(&format!("[con_id={f}] focus"));
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
    fn ipc_messages_follow_the_i3_format() {
        let m = encode(RUN_COMMAND, "create_output");
        assert_eq!(&m[..6], b"i3-ipc");
        assert_eq!(u32::from_ne_bytes(m[6..10].try_into().unwrap()), 13);
        assert_eq!(
            u32::from_ne_bytes(m[10..14].try_into().unwrap()),
            RUN_COMMAND
        );
        assert_eq!(&m[14..], b"create_output");
    }

    #[test]
    fn coverage_from_the_tree() {
        let outputs = json!([
            { "name": "DP-1", "active": true, "power": true, "current_workspace": "1" },
            { "name": "DP-2", "active": true, "power": false, "current_workspace": "2" },
            { "name": "HEADLESS-1", "active": true, "current_workspace": "rustpaper-DP-1" },
        ]);
        let tree = json!({ "type": "root", "nodes": [
            { "type": "output", "name": "DP-1", "nodes": [
                { "type": "workspace", "name": "1", "nodes": [
                    { "type": "con", "pid": 10, "name": "term", "focused": true, "fullscreen_mode": 0 }
                ], "floating_nodes": [] }
            ]},
            { "type": "output", "name": "DP-2", "nodes": [
                { "type": "workspace", "name": "2", "nodes": [], "floating_nodes": [
                    { "type": "floating_con", "pid": 11, "name": "calc", "fullscreen_mode": 0 }
                ]}
            ]},
            { "type": "output", "name": "HEADLESS-1", "nodes": [
                { "type": "workspace", "name": "rustpaper-DP-1", "nodes": [
                    { "type": "con", "id": 42, "pid": 12, "name": "rustpaper-DP-1", "fullscreen_mode": 1 }
                ]}
            ]}
        ]});
        let c = covering(outputs.as_array().unwrap(), &tree);
        assert_eq!(c.len(), 2, "headless outputs are not monitors");
        assert!(
            c[0].maximized && c[0].focused && !c[0].fullscreen,
            "a tiled, focused window"
        );
        assert!(
            c[1].asleep && !c[1].maximized,
            "a floating window leaves the wallpaper visible"
        );
        assert_eq!(
            find_window(&tree, "rustpaper-DP-1").and_then(|w| w.get("id")),
            Some(&json!(42))
        );
    }
}
