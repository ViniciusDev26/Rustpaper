//! What plays on the host, for Wallpaper Engine's media integration (album
//! art, title, artist and playback state in scenes).
//!
//! WE reads it from Windows' media session API, which Proton lacks;
//! `rustpaper_media.dll` (crates/rustpaper-we-helper, whose lib.rs has the
//! trace) provides that API inside WE's prefix. This side:
//!
//! - copies the DLL where WE's container sees it (the cache in the home)
//!   and registers it in WE's prefix for
//!   `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`
//!   (`HKLM\Software\Microsoft\WindowsRuntime\ActivatableClassId\<class>`,
//!   `DllPath`, like Wine's own classes), once per WE process;
//! - follows the session bus's MPRIS players (`org.mpris.MediaPlayer2.*`:
//!   Spotify, browsers, mpv with mpv-mpris...) and writes the one playing
//!   into the media folder next to the DLL (`state`, `art-*.png`; the
//!   format is that crate's `format.rs`);
//! - carries out what WE asks of the player (`cmd-*` files: play, pause,
//!   next...).
//!
//! WE's helper (`winrtutil64.exe`) activates the class when WE starts it,
//! so a WE started before the class was registered gets it on its next
//! start; the engine then restarts it once (see `tend`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use dbus::arg::{PropMap, RefArg, prop_cast};
use dbus::blocking::Connection;
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use rustpaper_media::format::{self, Command, Kind, Repeat, Snapshot, Status, caps};

use crate::we;

const DLL: &str = "rustpaper_media.dll";
/// The classes the DLL provides in WE's prefix: the media sessions, and a
/// DataReader over the album art (Wine's is a stub for streams; the DLL
/// hands everything else to Wine's own, see its datareader.rs).
const CLASSES: [&str; 2] = [
    "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager",
    "Windows.Storage.Streams.DataReader",
];
const CLASS_KEY: &str = r"HKLM\Software\Microsoft\WindowsRuntime\ActivatableClassId";
const MPRIS: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const POLL: Duration = Duration::from_millis(500);
/// A position this far from where the last one should be now is a seek.
const SEEK_US: i64 = 2_000_000;
/// Album art larger than this (either side) is scaled down.
const ART_SIZE: u32 = 512;

/// The media folder: `media` next to the DLL, in the cache WE's container
/// shares.
pub fn dir() -> Option<PathBuf> {
    Some(we::cache_dir()?.join("media"))
}

/// The DLL and the MPRIS follower, while WE is in use.
#[derive(Default)]
pub struct MediaBridge {
    follower: Option<Follower>,
    /// WE process the class was registered for.
    registered_for: Option<u32>,
    /// Logged that the DLL is missing (once).
    missing: bool,
    /// Restarted WE for the class once already.
    restarted: bool,
}

impl MediaBridge {
    /// Called every tick while WE is in use: starts following the players,
    /// and installs and registers the DLL for a new WE process. Returns true
    /// after a first registration (a new install, or new classes): WE then
    /// has to start again to use them.
    pub fn tend(&mut self) -> bool {
        let Some(dir) = dir() else {
            return false;
        };
        if self.follower.is_none() {
            self.follower = Some(Follower::start(dir.clone()));
        }
        let Some(pid) = we::pid() else {
            return false;
        };
        if self.registered_for == Some(pid) {
            return false;
        }
        self.registered_for = Some(pid);
        let Some(dll) = self.install() else {
            return false;
        };
        let value = we::wine_path(&dll);
        // a marker of what was registered: the prefix's registry is not
        // read back (that would need WE's container for a reg query)
        let marker = dir.join("registered");
        let registration = format!("{value}\n{}\n", CLASSES.join("\n"));
        let first = std::fs::read_to_string(&marker).ok().as_deref() != Some(registration.as_str());
        if !first {
            // ~1 s per class; never block the watchdog on it
            std::thread::spawn(move || {
                if !register(&value) {
                    eprintln!("media: registering {DLL} in WE's prefix failed");
                }
            });
            return false;
        }
        // the first time, wait for it: a restart of WE would cut it short
        if !register(&value) {
            eprintln!("media: registering {DLL} in WE's prefix failed");
            return false;
        }
        let _ = std::fs::write(&marker, &registration);
        eprintln!("media: {DLL} registered for WE's media integration");
        // This WE started before (the registration needs its container),
        // and its helper keeps a factory it got once (C++/WinRT caches
        // them): only a restart makes it use these classes. Once per engine.
        let restart = !self.restarted;
        self.restarted = true;
        restart
    }

    /// Stops following the players (WE no longer used, or the engine exits).
    pub fn stop(&mut self) {
        self.follower = None;
        self.registered_for = None;
    }

    /// The DLL copied next to the media folder, replaced (written aside,
    /// then renamed: a running helper keeps the old file) when the installed
    /// one differs. None (logged once) when it is not installed.
    fn install(&mut self) -> Option<PathBuf> {
        let Some(src) = crate::we_helper::installed_file(DLL) else {
            if !self.missing {
                self.missing = true;
                eprintln!("media: {DLL} not found; WE's media integration stays off");
            }
            return None;
        };
        let dir = we::cache_dir()?;
        let dst = dir.join(DLL);
        let bytes = std::fs::read(&src).ok()?;
        if std::fs::read(&dst).ok().as_deref() != Some(bytes.as_slice()) {
            std::fs::create_dir_all(&dir).ok()?;
            let tmp = dir.join(format!(".{DLL}.tmp"));
            if let Err(e) = std::fs::write(&tmp, &bytes).and_then(|()| std::fs::rename(&tmp, &dst))
            {
                eprintln!("media: copying {DLL} to {}: {e}", dir.display());
                return None;
            }
        }
        Some(dst)
    }
}

/// Registers the DLL (its Wine path `value`) for [`CLASSES`] in WE's
/// prefix, through `reg add` in WE's container (20 s at most each).
fn register(value: &str) -> bool {
    CLASSES.iter().all(|class| {
        let key = format!(r"{CLASS_KEY}\{class}");
        let argv: Vec<String> = [
            "reg", "add", &key, "/v", "DllPath", "/t", "REG_SZ", "/d", value, "/f",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let Some(mut child) = we::spawn_in_we(&argv) else {
            return false;
        };
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match child.try_wait() {
                Ok(Some(s)) => return s.success(),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
            }
        }
    })
}

/// The thread following the MPRIS players.
struct Follower {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Follower {
    fn start(dir: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::Builder::new()
            .name("media".into())
            .spawn(move || follow(&dir, &s))
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn follow(dir: &Path, stop: &AtomicBool) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        eprintln!("media: {}: {e}", dir.display());
        return;
    }
    let mut writer = Writer::new(dir.to_path_buf());
    let mut picker = Picker::default();
    let mut conn: Option<Connection> = None;
    let mut retry = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        if conn.is_none() && Instant::now() >= retry {
            match Connection::new_session() {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    eprintln!("media: session bus: {e}");
                    retry = Instant::now() + Duration::from_secs(30);
                }
            }
        }
        let players = conn.as_ref().map(players).unwrap_or_default();
        let chosen = picker.pick(&players);
        let (snapshot, art) =
            chosen.map_or_else(|| (Snapshot::default(), String::new()), |p| p.snapshot());
        writer.update(snapshot, &art);
        if let Some(c) = &conn {
            run_commands(c, dir, chosen);
        }
        std::thread::sleep(POLL);
    }
}

/// One MPRIS player as last read.
struct Player {
    bus: String,
    identity: String,
    props: PropMap,
}

/// Every MPRIS player on the session bus.
fn players(conn: &Connection) -> Vec<Player> {
    let bus = conn.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_secs(2),
    );
    let Ok((names,)): Result<(Vec<String>,), _> =
        bus.method_call("org.freedesktop.DBus", "ListNames", ())
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = names
        .into_iter()
        .filter(|n| n.starts_with("org.mpris.MediaPlayer2."))
        .collect();
    names.sort();
    names
        .into_iter()
        .filter_map(|n| {
            let p = conn.with_proxy(
                n.as_str(),
                "/org/mpris/MediaPlayer2",
                Duration::from_secs(1),
            );
            let props = p.get_all(PLAYER).ok()?;
            let identity: String = p.get(MPRIS, "Identity").unwrap_or_else(|_| {
                n.trim_start_matches("org.mpris.MediaPlayer2.")
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            });
            Some(Player {
                bus: n,
                identity,
                props,
            })
        })
        .collect()
}

fn str_of<'a>(m: &'a PropMap, key: &str) -> Option<&'a str> {
    m.get(key).and_then(|v| v.0.as_str())
}

fn bool_of(m: &PropMap, key: &str) -> Option<bool> {
    prop_cast::<bool>(m, key).copied()
}

fn int_of(m: &PropMap, key: &str) -> Option<i64> {
    m.get(key)
        .and_then(|v| v.0.as_i64().or_else(|| v.0.as_u64().map(|u| u as i64)))
}

/// A string list (`as`), or a lone string some players send instead.
fn strings_of(m: &PropMap, key: &str) -> Vec<String> {
    if let Some(v) = prop_cast::<Vec<String>>(m, key) {
        return v.clone();
    }
    str_of(m, key)
        .map(|s| vec![s.to_string()])
        .unwrap_or_default()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

impl Player {
    fn status(&self) -> Status {
        match str_of(&self.props, "PlaybackStatus") {
            Some("Playing") => Status::Playing,
            Some("Paused") => Status::Paused,
            _ => Status::Stopped,
        }
    }

    fn metadata(&self) -> Option<&PropMap> {
        prop_cast::<PropMap>(&self.props, "Metadata")
    }

    fn title(&self) -> &str {
        self.metadata()
            .and_then(|m| str_of(m, "xesam:title"))
            .unwrap_or_default()
    }

    /// The state for WE, and the album art's URL.
    fn snapshot(&self) -> (Snapshot, String) {
        let p = &self.props;
        let empty = PropMap::new();
        let m = self.metadata().unwrap_or(&empty);
        let control = bool_of(p, "CanControl").unwrap_or(true);
        let can = |key: &str| control && bool_of(p, key).unwrap_or(false);
        let mut c = 0;
        for (yes, bit) in [
            (can("CanPlay"), caps::PLAY),
            (can("CanPause"), caps::PAUSE),
            (can("CanPlay") || can("CanPause"), caps::TOGGLE),
            (control, caps::STOP),
            (can("CanGoNext"), caps::NEXT),
            (can("CanGoPrevious"), caps::PREVIOUS),
            (can("CanSeek"), caps::SEEK),
            (control && p.contains_key("Rate"), caps::RATE),
            (control && p.contains_key("Shuffle"), caps::SHUFFLE),
            (control && p.contains_key("LoopStatus"), caps::REPEAT),
        ] {
            if yes {
                c |= bit;
            }
        }
        let artist = strings_of(m, "xesam:artist").join(", ");
        let album = str_of(m, "xesam:album").unwrap_or_default().to_string();
        let s = Snapshot {
            status: self.status(),
            app: self.identity.clone(),
            title: self.title().to_string(),
            kind: if artist.is_empty() && album.is_empty() {
                Kind::Unknown
            } else {
                Kind::Music
            },
            artist,
            album,
            album_artist: strings_of(m, "xesam:albumArtist").join(", "),
            track: int_of(m, "xesam:trackNumber").unwrap_or(0) as i32,
            genres: strings_of(m, "xesam:genre"),
            length_us: int_of(m, "mpris:length").unwrap_or(0).max(0),
            position_us: int_of(p, "Position").unwrap_or(0).max(0),
            updated_ms: now_ms(),
            rate: p.get("Rate").and_then(|v| v.0.as_f64()).unwrap_or(1.0),
            shuffle: bool_of(p, "Shuffle"),
            repeat: str_of(p, "LoopStatus").map(|l| match l {
                "Track" => Repeat::Track,
                "Playlist" => Repeat::List,
                _ => Repeat::None,
            }),
            caps: c,
            ..Snapshot::default()
        };
        (s, str_of(m, "mpris:artUrl").unwrap_or_default().to_string())
    }
}

/// Which player is "the" session: the one playing (the same one as long as
/// it plays; else the one that started playing last), else the last one
/// chosen while it stays paused, else any paused one. A stopped player
/// without a track is no session.
#[derive(Default)]
struct Picker {
    last: Option<String>,
    /// When each player was last seen starting to play.
    started: HashMap<String, Instant>,
    playing: HashMap<String, bool>,
}

impl Picker {
    fn pick<'a>(&mut self, players: &'a [Player]) -> Option<&'a Player> {
        let now = Instant::now();
        for p in players {
            let playing = p.status() == Status::Playing;
            if playing && !self.playing.get(&p.bus).copied().unwrap_or(false) {
                self.started.insert(p.bus.clone(), now);
            }
            self.playing.insert(p.bus.clone(), playing);
        }
        self.playing
            .retain(|b, _| players.iter().any(|p| &p.bus == b));
        self.started
            .retain(|b, _| players.iter().any(|p| &p.bus == b));
        let last = self
            .last
            .as_deref()
            .and_then(|l| players.iter().find(|p| p.bus == l));
        let chosen = last
            .filter(|p| p.status() == Status::Playing)
            .or_else(|| {
                players
                    .iter()
                    .filter(|p| p.status() == Status::Playing)
                    .max_by_key(|p| self.started.get(&p.bus).copied())
            })
            .or_else(|| last.filter(|p| p.status() == Status::Paused))
            .or_else(|| players.iter().find(|p| p.status() == Status::Paused))
            .or_else(|| last.filter(|p| !p.title().is_empty()))
            .or_else(|| players.iter().find(|p| !p.title().is_empty()));
        self.last = chosen.map(|p| p.bus.clone());
        chosen
    }
}

/// Writes the state when it changes (not merely moving on in the track:
/// WE's side advances the position itself from when it was read), and the
/// album art when its URL changes.
struct Writer {
    dir: PathBuf,
    last: Option<Snapshot>,
    seq: u64,
    art_url: String,
    art_file: String,
    art_n: u64,
}

impl Writer {
    fn new(dir: PathBuf) -> Self {
        // seq keeps growing across engines (a new one only has to differ)
        let seq = now_ms() as u64;
        Self {
            dir,
            last: None,
            seq,
            art_url: String::new(),
            art_file: String::new(),
            art_n: seq,
        }
    }

    fn update(&mut self, mut s: Snapshot, art_url: &str) {
        if art_url != self.art_url {
            self.art_url = art_url.to_string();
            self.art_n += 1;
            self.art_file = save_art(&self.dir, art_url, self.art_n).unwrap_or_default();
        }
        s.art = if s.present() {
            self.art_file.clone()
        } else {
            String::new()
        };
        let write = match &self.last {
            None => true,
            Some(l) => !l.same_but_position(&s) || seeked(l, &s),
        };
        if !write {
            return;
        }
        self.seq += 1;
        s.seq = self.seq;
        let tmp = self.dir.join(".state.tmp");
        let dst = self.dir.join(format::STATE);
        if let Err(e) = std::fs::write(&tmp, s.write()).and_then(|()| std::fs::rename(&tmp, &dst)) {
            eprintln!("media: {}: {e}", dst.display());
            return;
        }
        self.last = Some(s);
    }
}

/// The position moved other than by playing on (a seek, or a new track
/// with the same metadata).
fn seeked(last: &Snapshot, now: &Snapshot) -> bool {
    let elapsed_us = (now.updated_ms - last.updated_ms) * 1000;
    let expected = if last.status == Status::Playing {
        last.position_us + (elapsed_us as f64 * last.rate) as i64
    } else {
        last.position_us
    };
    (now.position_us - expected).abs() > SEEK_US
}

/// Fetches the album art at `url` (file:// or http(s)://, the latter
/// through curl) and saves it as `art-<n>.png` (scaled down to
/// [`ART_SIZE`]), removing the previous ones. The file name, or None.
fn save_art(dir: &Path, url: &str, n: u64) -> Option<String> {
    let old: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|f| f.to_str())
                        .is_some_and(|f| f.starts_with("art-") && f.ends_with(".png"))
                })
                .collect()
        })
        .unwrap_or_default();
    let name = (|| {
        let bytes = fetch(url)?;
        let img = image::load_from_memory(&bytes)
            .map_err(|e| eprintln!("media: album art {url}: {e}"))
            .ok()?;
        let img = if img.width() > ART_SIZE || img.height() > ART_SIZE {
            img.thumbnail(ART_SIZE, ART_SIZE)
        } else {
            img
        };
        let name = format!("art-{n}.png");
        img.save_with_format(dir.join(&name), image::ImageFormat::Png)
            .map_err(|e| eprintln!("media: album art: {e}"))
            .ok()?;
        Some(name)
    })();
    for p in old {
        let _ = std::fs::remove_file(p);
    }
    name
}

/// The bytes at a file:// or http(s):// URL.
fn fetch(url: &str) -> Option<Vec<u8>> {
    if let Some(path) = url.strip_prefix("file://") {
        return std::fs::read(percent_decode(path)).ok();
    }
    if url.starts_with("https://") || url.starts_with("http://") {
        let out = std::process::Command::new("curl")
            .args([
                "-sfL",
                "--max-time",
                "10",
                "--max-filesize",
                "20000000",
                "--",
                url,
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .map_err(|e| eprintln!("media: curl: {e} (album art from the web needs curl)"))
            .ok()?;
        return (out.status.success() && !out.stdout.is_empty()).then_some(out.stdout);
    }
    None
}

/// A file URL's path with `%XX` escapes decoded.
fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Carries out (and removes) the `cmd-*` files WE's side wrote, on the
/// chosen player.
fn run_commands(conn: &Connection, dir: &Path, player: Option<&Player>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| f.starts_with(format::COMMAND_PREFIX))
        })
        .collect();
    files.sort();
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap_or_default();
        let _ = std::fs::remove_file(&f);
        let (Some(cmd), Some(p)) = (Command::parse(&text), player) else {
            continue;
        };
        if let Err(e) = command(conn, p, cmd) {
            eprintln!("media: {} {cmd:?}: {e}", p.identity);
        }
    }
}

fn command(conn: &Connection, p: &Player, cmd: Command) -> Result<(), dbus::Error> {
    let proxy = conn.with_proxy(
        p.bus.as_str(),
        "/org/mpris/MediaPlayer2",
        Duration::from_secs(2),
    );
    let call = |method: &str| proxy.method_call::<(), _, _, _>(PLAYER, method, ());
    match cmd {
        Command::Play => call("Play"),
        Command::Pause => call("Pause"),
        Command::Toggle => call("PlayPause"),
        Command::Stop => call("Stop"),
        Command::Next => call("Next"),
        Command::Previous => call("Previous"),
        Command::Seek(us) => {
            let track = p
                .metadata()
                .and_then(|m| m.get("mpris:trackid"))
                .and_then(|v| v.0.as_str())
                .and_then(|t| dbus::Path::new(t.to_string()).ok())
                .unwrap_or_else(|| dbus::Path::from("/org/mpris/MediaPlayer2/TrackList/NoTrack"));
            proxy.method_call(PLAYER, "SetPosition", (track, us))
        }
        Command::Rate(r) => proxy.set(PLAYER, "Rate", r),
        Command::Shuffle(on) => proxy.set(PLAYER, "Shuffle", on),
        Command::Repeat(r) => proxy.set(
            PLAYER,
            "LoopStatus",
            match r {
                Repeat::None => "None",
                Repeat::Track => "Track",
                Repeat::List => "Playlist",
            }
            .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_are_decoded() {
        assert_eq!(percent_decode("/tmp/a%20b/c%C3%A9.png"), "/tmp/a b/cé.png");
        assert_eq!(percent_decode("/x/100%"), "/x/100%");
        assert_eq!(percent_decode("/x/%zz"), "/x/%zz");
    }

    #[test]
    fn only_a_jump_in_the_position_is_a_seek() {
        let last = Snapshot {
            status: Status::Playing,
            position_us: 10_000_000,
            updated_ms: 1_000,
            rate: 1.0,
            ..Snapshot::default()
        };
        let on = |pos, at| Snapshot {
            position_us: pos,
            updated_ms: at,
            ..last.clone()
        };
        assert!(!seeked(&last, &on(15_000_000, 6_000)), "played on 5 s");
        assert!(seeked(&last, &on(60_000_000, 6_000)), "jumped ahead");
        assert!(seeked(&last, &on(0, 6_000)), "back to the start");
        let paused = Snapshot {
            status: Status::Paused,
            ..last.clone()
        };
        assert!(!seeked(&paused, &on(10_000_000, 60_000)), "paused stays");
    }

    #[test]
    fn state_files_are_written_on_changes_only() {
        let dir = std::env::temp_dir().join(format!("rp-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut w = Writer::new(dir.clone());
        let playing = Snapshot {
            status: Status::Playing,
            title: "A".into(),
            position_us: 0,
            updated_ms: 1_000,
            ..Snapshot::default()
        };
        w.update(playing.clone(), "");
        let read =
            || Snapshot::parse(&std::fs::read_to_string(dir.join(format::STATE)).unwrap()).unwrap();
        let first = read();
        assert_eq!(first.title, "A");
        // 1 s later, 1 s further: nothing new
        w.update(
            Snapshot {
                position_us: 1_000_000,
                updated_ms: 2_000,
                ..playing.clone()
            },
            "",
        );
        assert_eq!(read().seq, first.seq);
        w.update(
            Snapshot {
                title: "B".into(),
                ..playing.clone()
            },
            "",
        );
        assert_eq!(read().title, "B");
        assert!(read().seq > first.seq);
        // a player gone: no session, and no art left behind
        w.update(Snapshot::default(), "");
        assert!(!read().present());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
