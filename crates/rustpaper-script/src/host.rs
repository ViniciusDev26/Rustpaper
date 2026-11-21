// The renderer's side of the sandbox: starts the worker and talks to it.
//
// The worker gets an empty environment (TZ only, for local time), no file
// descriptors but its three pipes, and is killed when the host drops it, when
// it answers late, or when it sends anything that doesn't parse. A dead
// worker never takes the scene down: the scene just keeps its static values.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::Duration;

use crate::protocol::{Frame, Init, Reply};

/// Loading every module (each has its own 500 ms budget in the worker).
const INIT_TIMEOUT: Duration = Duration::from_secs(15);
/// First frame: init() + applyUserProperties() + update().
const START_TIMEOUT: Duration = Duration::from_secs(3);
/// Every other frame (the worker's own budget is 40 ms).
const FRAME_TIMEOUT: Duration = Duration::from_millis(250);
/// Worker stderr lines forwarded (panics, sandbox setup errors).
const MAX_STDERR_LINES: usize = 50;
/// Longest reply line accepted from the worker.
const MAX_REPLY: usize = 4 << 20;

pub struct ScriptHost {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
    started: bool,
}

impl ScriptHost {
    /// Starts the worker, sends `init` and waits for its load report.
    /// `utc_offset_min`: the local time zone (minutes east of UTC).
    pub fn spawn(init: &Init, utc_offset_min: i32) -> Result<(ScriptHost, Reply), String> {
        let exe = worker_path().ok_or("rustpaper-script-worker not found")?;
        Self::spawn_worker(&exe, init, utc_offset_min)
    }

    /// `spawn` with an explicit worker executable.
    pub fn spawn_worker(
        exe: &std::path::Path,
        init: &Init,
        utc_offset_min: i32,
    ) -> Result<(ScriptHost, Reply), String> {
        let mut child = Command::new(exe)
            .env_clear()
            .env("TZ", posix_tz(utc_offset_min))
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // piped, never inherited: the jail forbids writing files
            // (RLIMIT_FSIZE 0), and the renderer's stderr may be one
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))?;
        if let Some(err) = child.stderr.take() {
            let _ = std::thread::Builder::new()
                .name("script-worker-stderr".into())
                .spawn(move || {
                    let r = BufReader::new(err);
                    for line in r.lines().map_while(Result::ok).take(MAX_STDERR_LINES) {
                        eprintln!("[script worker] {line}");
                    }
                });
        }
        let stdin = child.stdin.take().ok_or("worker stdin")?;
        let stdout = child.stdout.take().ok_or("worker stdout")?;
        let (tx, replies) = channel();
        std::thread::Builder::new()
            .name("script-worker-reader".into())
            .spawn(move || {
                let mut r = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    // bounded read: a runaway line ends the conversation
                    match Read::by_ref(&mut r)
                        .take(MAX_REPLY as u64)
                        .read_line(&mut line)
                    {
                        Ok(0) | Err(_) => return,
                        Ok(_) if !line.ends_with('\n') => return,
                        Ok(_) => {
                            if tx.send(line.trim_end().to_owned()).is_err() {
                                return;
                            }
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let mut host = ScriptHost {
            child,
            stdin,
            replies,
            started: false,
        };
        let msg = serde_json::to_string(init).map_err(|e| e.to_string())?;
        let reply = host.exchange(&msg, INIT_TIMEOUT)?;
        Ok((host, reply))
    }

    /// Runs every script's update() for this frame. An error means the
    /// worker is gone (killed): drop the host and keep the static values.
    pub fn frame(&mut self, frame: &Frame) -> Result<Reply, String> {
        let msg = serde_json::to_string(frame).map_err(|e| e.to_string())?;
        let timeout = if self.started {
            FRAME_TIMEOUT
        } else {
            START_TIMEOUT
        };
        self.started = true;
        self.exchange(&msg, timeout)
    }

    /// The worker's process id.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    fn exchange(&mut self, msg: &str, timeout: Duration) -> Result<Reply, String> {
        let sent = self
            .stdin
            .write_all(msg.as_bytes())
            .and_then(|_| self.stdin.write_all(b"\n"))
            .and_then(|_| self.stdin.flush());
        if let Err(e) = sent {
            self.kill();
            return Err(format!("script worker: {e}"));
        }
        match self.replies.recv_timeout(timeout) {
            Ok(line) => serde_json::from_str(&line).map_err(|e| {
                self.kill();
                format!("script worker sent garbage: {e}")
            }),
            Err(RecvTimeoutError::Timeout) => {
                self.kill();
                Err(format!("script worker did not answer within {timeout:?}"))
            }
            Err(RecvTimeoutError::Disconnected) => {
                let status = self.child.wait().ok();
                Err(format!("script worker exited ({status:?})"))
            }
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ScriptHost {
    fn drop(&mut self) {
        self.kill();
    }
}

/// `RUSTPAPER_SCRIPT_WORKER`, else next to the running executable, else PATH.
fn worker_path() -> Option<PathBuf> {
    const NAME: &str = "rustpaper-script-worker";
    if let Some(p) = std::env::var_os("RUSTPAPER_SCRIPT_WORKER") {
        return Some(PathBuf::from(p));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(PathBuf::from))
    {
        let p = dir.join(NAME);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| PathBuf::from(d).join(NAME))
        .find(|p| p.is_file())
}

/// POSIX TZ string for a fixed offset (no zone file needed: the sandbox can't
/// open files). POSIX counts hours WEST of UTC, so the sign flips; the zone
/// name needs at least 3 letters or glibc falls back to UTC.
fn posix_tz(utc_offset_min: i32) -> String {
    let west = -utc_offset_min;
    let sign = if west < 0 { "-" } else { "" };
    let a = west.unsigned_abs();
    format!("RPT{sign}{}:{:02}", a / 60, a % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn posix_tz_flips_the_sign() {
        assert_eq!(posix_tz(-180), "RPT3:00");
        assert_eq!(posix_tz(330), "RPT-5:30");
        assert_eq!(posix_tz(0), "RPT0:00");
    }
}
