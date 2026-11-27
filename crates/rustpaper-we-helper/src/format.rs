//! The files the engine (`media.rs`) and `rustpaper_media.dll` exchange in
//! the media folder (`~/.cache/rustpaper/media`, which WE's container
//! shares; the DLL finds it next to itself):
//!
//! - `state`: what plays now (a [`Snapshot`]), written by the engine
//!   (atomically: written aside, then renamed) whenever it changes;
//! - `art-<seq>.png`: the album art the state names;
//! - `cmd-<n>`: one [`Command`] each, from WE (play, pause, next...),
//!   written by the DLL and carried out and removed by the engine.
//!
//! Text, one `key=value` per line, values escaped (`\\`, `\n`, `\r`), so
//! the no_std DLL parses it without a JSON parser. Unknown keys are
//! skipped: either side may be newer.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// First line of the state file.
pub const HEADER: &str = "rustpaper-media 1";
/// The state file, in the media folder.
pub const STATE: &str = "state";
/// Prefix of the command files, in the media folder.
pub const COMMAND_PREFIX: &str = "cmd-";

/// A player's state, as Windows' GlobalSystemMediaTransportControls
/// reports it (`GlobalSystemMediaTransportControlsSessionPlaybackStatus`,
/// minus `Closed`/`Opened`): `None` means no player at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    None,
    Changing,
    Stopped,
    Playing,
    Paused,
}

/// `Windows.Media.MediaPlaybackType`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Unknown,
    Music,
    Video,
}

/// `Windows.Media.MediaPlaybackAutoRepeatMode` (MPRIS `LoopStatus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    None,
    Track,
    List,
}

/// What the player accepts (bits of [`Snapshot::caps`]).
pub mod caps {
    pub const PLAY: u16 = 1;
    pub const PAUSE: u16 = 1 << 1;
    pub const STOP: u16 = 1 << 2;
    pub const NEXT: u16 = 1 << 3;
    pub const PREVIOUS: u16 = 1 << 4;
    pub const SEEK: u16 = 1 << 5;
    pub const TOGGLE: u16 = 1 << 6;
    pub const RATE: u16 = 1 << 7;
    pub const SHUFFLE: u16 = 1 << 8;
    pub const REPEAT: u16 = 1 << 9;
}

/// What plays now.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Grows with every write.
    pub seq: u64,
    pub status: Status,
    /// The player's name (MPRIS `Identity`).
    pub app: String,
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track: i32,
    pub tracks: i32,
    pub genres: Vec<String>,
    pub kind: Kind,
    /// Track length, 0 when unknown.
    pub length_us: i64,
    /// Position at `updated_ms`.
    pub position_us: i64,
    /// When the position was read (Unix time, ms).
    pub updated_ms: i64,
    pub rate: f64,
    pub shuffle: Option<bool>,
    pub repeat: Option<Repeat>,
    pub caps: u16,
    /// File name of the album art in the media folder, empty for none.
    pub art: String,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            seq: 0,
            status: Status::None,
            app: String::new(),
            title: String::new(),
            subtitle: String::new(),
            artist: String::new(),
            album: String::new(),
            album_artist: String::new(),
            track: 0,
            tracks: 0,
            genres: Vec::new(),
            kind: Kind::Unknown,
            length_us: 0,
            position_us: 0,
            updated_ms: 0,
            rate: 1.0,
            shuffle: None,
            repeat: None,
            caps: 0,
            art: String::new(),
        }
    }
}

fn escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn unescape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            Some(c) => o.push(c),
            None => {}
        }
    }
    o
}

const fn status_name(s: Status) -> &'static str {
    match s {
        Status::None => "none",
        Status::Changing => "changing",
        Status::Stopped => "stopped",
        Status::Playing => "playing",
        Status::Paused => "paused",
    }
}

const fn repeat_name(r: Repeat) -> &'static str {
    match r {
        Repeat::None => "none",
        Repeat::Track => "track",
        Repeat::List => "list",
    }
}

fn repeat_of(s: &str) -> Option<Repeat> {
    match s {
        "none" => Some(Repeat::None),
        "track" => Some(Repeat::Track),
        "list" => Some(Repeat::List),
        _ => None,
    }
}

impl Snapshot {
    /// Whether a player is there (a session, for Windows).
    pub fn present(&self) -> bool {
        self.status != Status::None
    }

    /// The state file's text.
    pub fn write(&self) -> String {
        let kind = match self.kind {
            Kind::Unknown => "unknown",
            Kind::Music => "music",
            Kind::Video => "video",
        };
        let opt_bool = |b: Option<bool>| match b {
            Some(true) => "1",
            Some(false) => "0",
            None => "",
        };
        let genres: Vec<String> = self.genres.iter().map(|g| escape(g)).collect();
        format!(
            "{HEADER}\nseq={}\nstatus={}\napp={}\ntitle={}\nsubtitle={}\nartist={}\nalbum={}\nalbum_artist={}\ntrack={}\ntracks={}\ngenres={}\nkind={kind}\nlength_us={}\nposition_us={}\nupdated_ms={}\nrate={}\nshuffle={}\nrepeat={}\ncaps={}\nart={}\n",
            self.seq,
            status_name(self.status),
            escape(&self.app),
            escape(&self.title),
            escape(&self.subtitle),
            escape(&self.artist),
            escape(&self.album),
            escape(&self.album_artist),
            self.track,
            self.tracks,
            // a tab never appears in a genre name worth keeping
            genres.join("\t"),
            self.length_us,
            self.position_us,
            self.updated_ms,
            self.rate,
            opt_bool(self.shuffle),
            self.repeat.map_or("", repeat_name),
            self.caps,
            escape(&self.art),
        )
    }

    /// Parses a state file; None when it is not one.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != HEADER {
            return None;
        }
        let mut s = Self::default();
        for line in lines {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "seq" => s.seq = v.parse().unwrap_or(0),
                "status" => {
                    s.status = match v {
                        "changing" => Status::Changing,
                        "stopped" => Status::Stopped,
                        "playing" => Status::Playing,
                        "paused" => Status::Paused,
                        _ => Status::None,
                    }
                }
                "app" => s.app = unescape(v),
                "title" => s.title = unescape(v),
                "subtitle" => s.subtitle = unescape(v),
                "artist" => s.artist = unescape(v),
                "album" => s.album = unescape(v),
                "album_artist" => s.album_artist = unescape(v),
                "track" => s.track = v.parse().unwrap_or(0),
                "tracks" => s.tracks = v.parse().unwrap_or(0),
                "genres" => {
                    s.genres = v
                        .split('\t')
                        .filter(|g| !g.is_empty())
                        .map(unescape)
                        .collect()
                }
                "kind" => {
                    s.kind = match v {
                        "music" => Kind::Music,
                        "video" => Kind::Video,
                        _ => Kind::Unknown,
                    }
                }
                "length_us" => s.length_us = v.parse().unwrap_or(0),
                "position_us" => s.position_us = v.parse().unwrap_or(0),
                "updated_ms" => s.updated_ms = v.parse().unwrap_or(0),
                "rate" => s.rate = v.parse().unwrap_or(1.0),
                "shuffle" => {
                    s.shuffle = match v {
                        "1" => Some(true),
                        "0" => Some(false),
                        _ => None,
                    }
                }
                "repeat" => s.repeat = repeat_of(v),
                "caps" => s.caps = v.parse().unwrap_or(0),
                "art" => s.art = unescape(v),
                _ => {}
            }
        }
        Some(s)
    }

    /// The same track, player and playback, ignoring where in the track it
    /// is: what decides whether the state is worth writing again.
    pub fn same_but_position(&self, o: &Self) -> bool {
        let strip = |s: &Self| Self {
            seq: 0,
            position_us: 0,
            updated_ms: 0,
            ..s.clone()
        };
        strip(self) == strip(o)
    }
}

/// What WE asks of the player (the session's `Try*Async`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Toggle,
    Stop,
    Next,
    Previous,
    /// Go to this position (µs).
    Seek(i64),
    Rate(f64),
    Shuffle(bool),
    Repeat(Repeat),
}

impl Command {
    /// A command file's text.
    pub fn write(&self) -> String {
        match self {
            Self::Play => "play".into(),
            Self::Pause => "pause".into(),
            Self::Toggle => "toggle".into(),
            Self::Stop => "stop".into(),
            Self::Next => "next".into(),
            Self::Previous => "previous".into(),
            Self::Seek(us) => format!("seek {us}"),
            Self::Rate(r) => format!("rate {r}"),
            Self::Shuffle(b) => format!("shuffle {}", u8::from(*b)),
            Self::Repeat(r) => format!("repeat {}", repeat_name(*r)),
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (verb, arg) = text.split_once(' ').unwrap_or((text, ""));
        Some(match verb {
            "play" => Self::Play,
            "pause" => Self::Pause,
            "toggle" => Self::Toggle,
            "stop" => Self::Stop,
            "next" => Self::Next,
            "previous" => Self::Previous,
            "seek" => Self::Seek(arg.parse().ok()?),
            "rate" => Self::Rate(arg.parse().ok()?),
            "shuffle" => Self::Shuffle(arg == "1"),
            "repeat" => Self::Repeat(repeat_of(arg)?),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn state_round_trips() {
        let s = Snapshot {
            seq: 42,
            status: Status::Playing,
            app: "Spotify".into(),
            title: "Line one\nline two \\ back".into(),
            artist: "Ação = sim".into(),
            album: "Álbum".into(),
            track: 3,
            tracks: 12,
            genres: vec!["Rock".into(), "Pop".into()],
            kind: Kind::Music,
            length_us: 215_000_000,
            position_us: 12_345_678,
            updated_ms: 1_800_000_000_000,
            rate: 1.25,
            shuffle: Some(true),
            repeat: Some(Repeat::List),
            caps: caps::PLAY | caps::PAUSE | caps::NEXT,
            art: "art-42.png".into(),
            ..Snapshot::default()
        };
        assert_eq!(Snapshot::parse(&s.write()), Some(s.clone()));
        let none = Snapshot::default();
        assert_eq!(Snapshot::parse(&none.write()), Some(none));
        assert_eq!(Snapshot::parse("something else\nseq=1"), None);
        // newer writers may add keys
        let mut text = s.write();
        text.push_str("future=1\n");
        assert_eq!(Snapshot::parse(&text).unwrap().title, s.title);
    }

    #[test]
    fn only_moving_on_in_the_track_is_not_a_change() {
        let a = Snapshot {
            status: Status::Playing,
            title: "A".into(),
            ..Snapshot::default()
        };
        let b = Snapshot {
            seq: 9,
            position_us: 5_000_000,
            updated_ms: 77,
            ..a.clone()
        };
        assert!(a.same_but_position(&b));
        let c = Snapshot {
            status: Status::Paused,
            ..a.clone()
        };
        assert!(!a.same_but_position(&c));
    }

    #[test]
    fn commands_round_trip() {
        for c in [
            Command::Play,
            Command::Pause,
            Command::Toggle,
            Command::Stop,
            Command::Next,
            Command::Previous,
            Command::Seek(90_000_000),
            Command::Rate(1.5),
            Command::Shuffle(true),
            Command::Shuffle(false),
            Command::Repeat(Repeat::Track),
        ] {
            assert_eq!(Command::parse(&c.write()), Some(c));
        }
        assert_eq!(Command::parse("fly"), None);
        assert_eq!(Command::parse("seek x"), None);
    }
}
