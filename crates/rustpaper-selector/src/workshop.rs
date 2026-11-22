// The selector side of the Steam Workshop store: spawns `rustpaper-workshop`
// (the Steamworks bridge, see that crate) on the first request and forwards
// JSON requests to it, matching replies by id.
//
// The helper exits on its own after a few idle minutes (Steam shows the user
// "playing Wallpaper Engine" while it runs) and when the selector goes away
// (its stdin closes), so a dead helper is simply spawned again.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

/// Longest wait for one reply (a Workshop query is usually < 2 s).
const TIMEOUT: Duration = Duration::from_secs(30);

type Pending = Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>;

struct Helper {
    child: Child,
    stdin: ChildStdin,
    pending: Pending,
}

#[derive(Default)]
pub struct Workshop {
    helper: Mutex<Option<Helper>>,
    next_id: Mutex<u64>,
    /// Helper executable; None: next to the selector, else PATH.
    exe: Option<PathBuf>,
}

impl Workshop {
    /// Sends `cmd` (a request without its id) and waits for the reply.
    /// Blocking: call it off the UI thread.
    pub fn request(&self, mut cmd: Value) -> Result<Value, String> {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        cmd["id"] = json!(id);
        let (tx, rx) = mpsc::channel();
        {
            let mut guard = self.helper.lock().unwrap();
            let alive = guard
                .as_mut()
                .is_some_and(|h| matches!(h.child.try_wait(), Ok(None)));
            if !alive {
                let exe = self.exe.clone().unwrap_or_else(helper_path);
                *guard = Some(spawn(&exe)?);
            }
            let h = guard.as_mut().expect("helper");
            h.pending.lock().unwrap().insert(id, tx);
            let line = format!("{cmd}\n");
            if h.stdin
                .write_all(line.as_bytes())
                .and_then(|_| h.stdin.flush())
                .is_err()
            {
                *guard = None;
                return Err("the Workshop helper stopped; try again".into());
            }
        }
        rx.recv_timeout(TIMEOUT)
            .map_err(|_| "Steam did not answer in time".to_string())?
    }
}

/// `rustpaper-workshop` next to the selector (target/ or the install
/// prefix), else from PATH.
fn helper_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.join("rustpaper-workshop")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("rustpaper-workshop"))
}

/// Starts the helper and waits for its `ready` line (Steam API up).
fn spawn(exe: &std::path::Path) -> Result<Helper, String> {
    let mut child = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // the Steam API logs the user's SteamID on stderr: not forwarded
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("failed to start rustpaper-workshop: {e}"))?;
    let stdin = child.stdin.take().ok_or("helper stdin")?;
    let stdout = child.stdout.take().ok_or("helper stdout")?;
    let pending: Pending = Arc::default();
    let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
    let replies = pending.clone();
    std::thread::spawn(move || {
        let mut ready = Some(ready_tx);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if let Some(r) = v.get("ready") {
                if let Some(tx) = ready.take() {
                    let res = if r.as_bool() == Some(true) {
                        Ok(())
                    } else {
                        Err(v["error"]
                            .as_str()
                            .unwrap_or("Steam unavailable")
                            .to_string())
                    };
                    let _ = tx.send(res);
                }
                continue;
            }
            let Some(id) = v["id"].as_u64() else {
                continue;
            };
            let res = if v["ok"].as_bool() == Some(true) {
                Ok(v["result"].clone())
            } else {
                Err(v["error"].as_str().unwrap_or("request failed").to_string())
            };
            if let Some(tx) = replies.lock().unwrap().remove(&id) {
                let _ = tx.send(res);
            }
        }
        // EOF: dropping the senders fails every waiting request at once
        replies.lock().unwrap().clear();
    });
    match ready_rx.recv_timeout(TIMEOUT) {
        Ok(Ok(())) => Ok(Helper {
            child,
            stdin,
            pending,
        }),
        Ok(Err(e)) => {
            let _ = child.wait();
            Err(e)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            Err("rustpaper-workshop did not start (is Steam running?)".into())
        }
    }
}

/// After unsubscribing: Steam keeps the files until some later cleanup, so
/// the wallpaper would linger in the gallery. Removes
/// `<workshop>/<id>` (plain numeric ids only, never outside that folder).
pub fn remove_unsubscribed(id: &str) -> Result<(), String> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid workshop id {id:?}"));
    }
    let Some(base) = rustpaper_core::paths::workshop_dir() else {
        return Ok(());
    };
    let dir = base.join(id);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs Steam running (signed in, owning WE) and the helper built:
    /// `cargo build -p rustpaper-workshop && cargo test -p rustpaper-selector -- --ignored`
    #[test]
    #[ignore]
    fn talks_to_steam_through_the_helper() {
        let exe =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/rustpaper-workshop");
        let w = Workshop {
            exe: Some(exe),
            ..Workshop::default()
        };
        let page = w
            .request(json!({ "cmd": "query", "sort": "top", "kind": "scene" }))
            .unwrap();
        let items = page["items"].as_array().unwrap();
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| i["kind"] == "scene"));
        let id = items[0]["id"].as_str().unwrap();
        let st = w
            .request(json!({ "cmd": "status", "items": [id] }))
            .unwrap();
        assert!(st[id]["subscribed"].is_boolean());
        let subs = w.request(json!({ "cmd": "subscriptions" })).unwrap();
        assert!(
            subs.as_object()
                .unwrap()
                .values()
                .all(|t| t.as_u64().is_some())
        );
        let bad = w.request(json!({ "cmd": "subscribe", "item": "../x" }));
        assert!(bad.unwrap_err().contains("invalid workshop id"));
    }

    #[test]
    fn unsubscribed_folders_only_by_numeric_id() {
        assert!(remove_unsubscribed("../etc").is_err());
        assert!(remove_unsubscribed("").is_err());
        assert!(remove_unsubscribed("12/34").is_err());
    }
}
