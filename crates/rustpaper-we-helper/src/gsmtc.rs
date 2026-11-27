//! `Windows.Media.Control` (GlobalSystemMediaTransportControls, "GSMTC"),
//! the API WE's media integration reads what plays now from, fed by the
//! engine's `state` file instead of Windows' media sessions (format.rs).
//!
//! The classes and their vtables follow the Windows metadata (as in the
//! `windows` crate's `Windows::Media::Control`); parameterized interface
//! IDs (`IAsyncOperation<T>`, `IVectorView<T>`, `IReference<T>`) were
//! computed from their signatures the way the Windows Runtime does (SHA-1,
//! RFC 4122 v5; the algorithm checked against `IAsyncOperation<bool>` =
//! cdb5efb3-5788-509d-9be1-71ccb8a3362a). There is one session at most:
//! the player the engine picked.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;

use crate::com::{
    CLASS_E_CLASSNOTAVAILABLE, Class, E_BOUNDS, E_NOINTERFACE, E_POINTER, Event, Guid, Hresult,
    Hstring, IID_IBUFFERBYTEACCESS, Ptr, Res, S_FALSE, S_OK, SLOTS, Unk, VTable, completed,
    completed_with_progress, create, hstring, hstring_chars, notimpl, put, this, vcall, vtable,
};
use crate::format::{Command, Kind, Repeat, STATE, Snapshot, Status, caps};
use crate::sys::{
    Lock, Sleep, log, media_path, read_file, spawn, stamp, ticks, trace, write_media_file,
};

const MANAGER_CLASS: &str =
    "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager";

const IID_ACTIVATION_FACTORY: Guid = Guid::new(0x00000035_0000_0000_c000_000000000046);
const IID_STATICS: Guid = Guid::new(0x2050c4ee_11a0_57de_aed7_c97c70338245);
const IID_MANAGER: Guid = Guid::new(0xcace8eac_e86e_504a_ab31_5ff8ff1bce49);
const IID_SESSION: Guid = Guid::new(0x7148c835_9b14_5ae2_ab85_dc9b1c14e1a8);
const IID_PROPERTIES: Guid = Guid::new(0x68856cf6_adb4_54b2_ac16_05837907acb6);
const IID_CONTROLS: Guid = Guid::new(0x6501a3e6_bc7a_503a_bb1b_68f158f3fb03);
const IID_PLAYBACK: Guid = Guid::new(0x94b4b6cf_e8ba_51ad_87a7_c10ade106127);
const IID_TIMELINE: Guid = Guid::new(0xede34136_6f25_588d_8ecf_ea5b6735aaa5);
const IID_CURRENT_CHANGED: Guid = Guid::new(0x6969cb39_0bfa_5fe0_8d73_09cc5e5408e1);
const IID_SESSIONS_CHANGED: Guid = Guid::new(0xbbf0cd32_42c4_5a58_b317_f34bbfbd26e0);
const IID_MEDIA_CHANGED: Guid = Guid::new(0x7d3741cb_adf0_5cef_91ba_cfabcdd77678);
const IID_PLAYBACK_CHANGED: Guid = Guid::new(0x786756c2_bc0d_50a5_8807_054291fef139);
const IID_TIMELINE_CHANGED: Guid = Guid::new(0x29033a2f_c923_5a77_bcaf_055ff415ad32);
// IAsyncOperation<Manager>, <MediaProperties>, <bool>,
// <IRandomAccessStreamWithContentType>; IAsyncOperationWithProgress<IBuffer, u32>
const IID_ASYNC_MANAGER: Guid = Guid::new(0x3eec115e_7346_5c27_8c5f_da78514a277b);
const IID_ASYNC_PROPERTIES: Guid = Guid::new(0xb185e6f3_e0d8_51cb_913f_c98d48c93c46);
const IID_ASYNC_BOOL: Guid = Guid::new(0xcdb5efb3_5788_509d_9be1_71ccb8a3362a);
const IID_ASYNC_STREAM: Guid = Guid::new(0xc4a57c5e_32b0_55b3_ad13_ce1c23041ed6);
const IID_ASYNC_READ: Guid = Guid::new(0xd26b2819_897f_5c7d_84d6_56d796561431);
// IVectorView / IIterable / IIterator of sessions and of strings
const IID_VIEW_SESSIONS: Guid = Guid::new(0x9b2672da_5088_5a1d_acd9_a3fc5ef1cfa4);
const IID_ITERABLE_SESSIONS: Guid = Guid::new(0x6c594bfe_b3dd_5f1d_a78f_3a2d9e937ca3);
const IID_ITERATOR_SESSIONS: Guid = Guid::new(0xec2e1c43_67db_5985_b2fa_91fd6a9c1ebf);
const IID_VIEW_STRINGS: Guid = Guid::new(0x2f13c006_a03a_5f69_b090_75a43e33423e);
const IID_ITERABLE_STRINGS: Guid = Guid::new(0xe2fcc7c1_3bfc_5a0b_b2b0_72e769d1cb7e);
const IID_ITERATOR_STRINGS: Guid = Guid::new(0x8c304ebb_6615_50a4_8829_879ecd443236);
// IReference<MediaPlaybackType>, <MediaPlaybackAutoRepeatMode>, <f64>, <bool>
const IID_REF_KIND: Guid = Guid::new(0xe289f7d8_6ba7_50ab_9f13_6e4e51d15ca4);
const IID_REF_REPEAT: Guid = Guid::new(0x50a7f41f_58d5_5c4d_9475_8dd1acd65836);
const IID_REF_F64: Guid = Guid::new(0x2f2d6c29_5473_5f3e_92e7_96572bb990e2);
const IID_REF_BOOL: Guid = Guid::new(0x3c00fd60_2950_5939_a21a_2d12c5a01b8a);
// Windows.Storage.Streams
const IID_STREAM_REFERENCE: Guid = Guid::new(0x33ee3134_1dd6_4e3a_8067_d1c162e8642b);
const IID_STREAM_WITH_TYPE: Guid = Guid::new(0xcc254827_4b3d_438f_9232_10c76bc7e038);
const IID_RANDOM_ACCESS_STREAM: Guid = Guid::new(0x905a0fe1_bc53_11df_8c49_001e4fc686da);
const IID_INPUT_STREAM: Guid = Guid::new(0x905a0fe2_bc53_11df_8c49_001e4fc686da);
const IID_OUTPUT_STREAM: Guid = Guid::new(0x905a0fe6_bc53_11df_8c49_001e4fc686da);
const IID_CLOSABLE: Guid = Guid::new(0x30d5a829_7fa4_4026_83bb_d75bae4ea99e);
const IID_CONTENT_TYPE: Guid = Guid::new(0x97d098a5_3b99_4de9_88a5_e11d2f50c795);

/// 100 ns ticks from 1601 to 1970 (Windows' DateTime epoch).
const UNIX_EPOCH_TICKS: i64 = 116_444_736_000_000_000;

fn null() -> Ptr {
    core::ptr::null_mut()
}

// ---------------------------------------------------------------- state

/// What plays now (the last `state` file read).
static SNAPSHOT: Lock<Option<Arc<Snapshot>>> = Lock::new(None);

fn current() -> Arc<Snapshot> {
    SNAPSHOT.with(|s| s.clone()).unwrap_or_default()
}

/// The `state` file; an empty state when it is missing or unreadable.
fn read_state() -> Snapshot {
    read_file(&media_path(STATE))
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|t| Snapshot::parse(&t))
        .unwrap_or_default()
}

/// Takes the `state` file in, then tells the listeners what changed.
fn reload() {
    let new = Arc::new(read_state());
    let Some(old) = SNAPSHOT.with(|s| s.replace(new.clone())) else {
        return;
    };
    if old.seq == new.seq && old.status == new.status {
        return;
    }
    // As Windows does: no player, no session (GetCurrentSession null). A
    // session that stayed Closed without properties, or a thumbnail that is
    // an empty stream, changed nothing for WE: it never fires
    // `mediaThumbnailChanged` with `hasThumbnail` false (the event is about
    // the image changing), so a scene that hides its album art only that
    // way keeps showing an empty, white texture once nothing plays, as it
    // would on Windows (tried 2026-11-27).
    if old.present() != new.present() {
        let m = manager().ptr();
        // SAFETY: the manager singleton (slot 0).
        let o = unsafe { this::<Manager, 0>(m) };
        o.val.current.fire(m, args(IID_CURRENT_CHANGED).ptr());
        o.val.sessions.fire(m, args(IID_SESSIONS_CHANGED).ptr());
    }
    if !new.present() {
        return;
    }
    let appeared = !old.present();
    let s = session().ptr();
    // SAFETY: the session singleton (slot 0).
    let o = unsafe { this::<Session, 0>(s) };
    if appeared || !same_media(&old, &new) {
        o.val.media.fire(s, args(IID_MEDIA_CHANGED).ptr());
    }
    let playback = |x: &Snapshot| {
        (
            x.status,
            x.caps,
            x.rate.to_bits(),
            x.shuffle,
            x.repeat,
            x.kind,
        )
    };
    if appeared || playback(&old) != playback(&new) {
        o.val.playback.fire(s, args(IID_PLAYBACK_CHANGED).ptr());
    }
    let timeline = |x: &Snapshot| (x.length_us, x.position_us, x.updated_ms);
    if appeared || timeline(&old) != timeline(&new) {
        o.val.timeline.fire(s, args(IID_TIMELINE_CHANGED).ptr());
    }
}

/// The same media properties (what `MediaPropertiesChanged` is about).
fn same_media(a: &Snapshot, b: &Snapshot) -> bool {
    a.app == b.app
        && a.title == b.title
        && a.subtitle == b.subtitle
        && a.artist == b.artist
        && a.album == b.album
        && a.album_artist == b.album_artist
        && a.track == b.track
        && a.tracks == b.tracks
        && a.genres == b.genres
        && a.kind == b.kind
        && a.art == b.art
}

/// Watches the `state` file (its write time and size, every 250 ms).
unsafe extern "system" fn watch(_: *mut c_void) -> u32 {
    trace("watch");
    let path = media_path(STATE);
    let mut last = stamp(&path);
    loop {
        // SAFETY: plain sleep.
        unsafe { Sleep(250) };
        let now = stamp(&path);
        if now != last {
            last = now;
            reload();
        }
    }
}

/// Asks the engine to have the player do `c` (a `cmd-*` file), when the
/// player accepts it (`need`, one of [`caps`]).
fn command(c: Command, need: u16) -> bool {
    static N: Lock<u32> = Lock::new(0);
    let s = current();
    if !s.present() || s.caps & need == 0 {
        return false;
    }
    let n = N.with(|n| {
        *n = n.wrapping_add(1);
        *n
    });
    // names sort in the order they were asked
    let name = alloc::format!(
        "{}{:016}{:06}",
        crate::format::COMMAND_PREFIX,
        ticks(),
        n % 1_000_000
    );
    write_media_file(&name, c.write().as_bytes())
}

// ---------------------------------------------------------------- entry

/// The Windows Runtime asks this DLL for a class's factory (registered in
/// WE's prefix under `ActivatableClassId\<class>` by the engine).
///
/// # Safety
/// Called by combase with a valid HSTRING and out pointer.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetActivationFactory(class: Hstring, out: *mut Ptr) -> Hresult {
    trace("DllGetActivationFactory");
    // SAFETY: per the caller.
    let name = unsafe { hstring_chars(class) };
    let is = |class: &str| class.encode_utf16().eq(name.iter().copied());
    let factory = if is(MANAGER_CLASS) {
        create(Factory, 0)
    } else if is(crate::datareader::CLASS) {
        create(crate::datareader::Factory, 0)
    } else {
        // SAFETY: per the caller.
        unsafe { put(out, null()) };
        return CLASS_E_CLASSNOTAVAILABLE;
    };
    // SAFETY: per the caller.
    unsafe { put(out, factory) }
}

/// Never unloaded: the watching thread runs in it.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> Hresult {
    S_FALSE
}

// ---------------------------------------------------------------- factory

struct Factory;

impl Class for Factory {
    const NAME: &'static str = MANAGER_CLASS;
    fn vtables(&self) -> [*const c_void; SLOTS] {
        let s = STATICS_VT.ptr();
        [FACTORY_VT.ptr(), s, s, s, s, s]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        if *iid == IID_ACTIVATION_FACTORY {
            Some(0)
        } else if *iid == IID_STATICS {
            Some(1)
        } else {
            None
        }
    }
}

unsafe extern "system" fn request_async(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("request_async");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, completed(IID_ASYNC_MANAGER, Res::Obj(Some(manager())))) }
}

static FACTORY_VT: VTable<7> = vtable!(Factory, 0; notimpl!("ActivateInstance"));
static STATICS_VT: VTable<7> = vtable!(Factory, 1; request_async);

// ---------------------------------------------------------------- manager

struct Manager {
    current: Event,
    sessions: Event,
}

impl Class for Manager {
    const NAME: &'static str = MANAGER_CLASS;
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [MANAGER_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_MANAGER).then_some(0)
    }
}

/// The manager (one per process); the first call reads the state and
/// starts watching it.
fn manager() -> Unk {
    static MANAGER: Lock<Option<Unk>> = Lock::new(None);
    let (m, fresh) = MANAGER.with(|m| match m {
        Some(m) => (m.clone(), false),
        None => {
            let new = Unk::own(create(
                Manager {
                    current: Event::new(),
                    sessions: Event::new(),
                },
                0,
            ))
            .expect("allocated");
            *m = Some(new.clone());
            (new, true)
        }
    });
    if fresh {
        let s = read_state();
        log(&alloc::format!("started: {:?} \"{}\"", s.status, s.title));
        SNAPSHOT.with(|x| *x = Some(Arc::new(s)));
        spawn(watch);
    }
    m
}

unsafe extern "system" fn get_current_session(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("get_current_session");
    let s = current().present().then(|| session().out());
    // SAFETY: out pointer from the caller.
    unsafe { put(out, s.unwrap_or_else(null)) }
}

unsafe extern "system" fn get_sessions(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("get_sessions");
    let items = if current().present() {
        alloc::vec![Item::Obj(session())]
    } else {
        Vec::new()
    };
    let v = View {
        kind: ViewKind::Sessions,
        items: Arc::new(items),
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(v, 0)) }
}

unsafe extern "system" fn add_current_changed(p: Ptr, h: Ptr, token: *mut i64) -> Hresult {
    trace("add_current_changed");
    // SAFETY: our interface pointer; caller's arguments.
    unsafe { this::<Manager, 0>(p).val.current.add(h, token) }
}

unsafe extern "system" fn remove_current_changed(p: Ptr, token: i64) -> Hresult {
    trace("remove_current_changed");
    // SAFETY: our interface pointer.
    unsafe { this::<Manager, 0>(p) }.val.current.remove(token)
}

unsafe extern "system" fn add_sessions_changed(p: Ptr, h: Ptr, token: *mut i64) -> Hresult {
    trace("add_sessions_changed");
    // SAFETY: our interface pointer; caller's arguments.
    unsafe { this::<Manager, 0>(p).val.sessions.add(h, token) }
}

unsafe extern "system" fn remove_sessions_changed(p: Ptr, token: i64) -> Hresult {
    trace("remove_sessions_changed");
    // SAFETY: our interface pointer.
    unsafe { this::<Manager, 0>(p) }.val.sessions.remove(token)
}

static MANAGER_VT: VTable<12> = vtable!(Manager, 0;
    get_current_session, get_sessions,
    add_current_changed, remove_current_changed,
    add_sessions_changed, remove_sessions_changed);

// ---------------------------------------------------------------- session

struct Session {
    media: Event,
    playback: Event,
    timeline: Event,
}

impl Class for Session {
    const NAME: &'static str = "Windows.Media.Control.GlobalSystemMediaTransportControlsSession";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [SESSION_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_SESSION).then_some(0)
    }
}

/// The session (one per process: whatever player the engine picked).
fn session() -> Unk {
    static SESSION: Lock<Option<Unk>> = Lock::new(None);
    SESSION.with(|s| {
        s.get_or_insert_with(|| {
            Unk::own(create(
                Session {
                    media: Event::new(),
                    playback: Event::new(),
                    timeline: Event::new(),
                },
                0,
            ))
            .expect("allocated")
        })
        .clone()
    })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

unsafe extern "system" fn source_app(_: Ptr, out: *mut Hstring) -> Hresult {
    trace("source_app");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, hstring(&wide(&current().app))) }
}

unsafe extern "system" fn try_get_properties(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("try_get_properties");
    let p = Unk::own(create(Properties(current()), 0));
    // SAFETY: out pointer from the caller.
    unsafe { put(out, completed(IID_ASYNC_PROPERTIES, Res::Obj(p))) }
}

unsafe extern "system" fn get_timeline(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("get_timeline");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(Timeline(current()), 0)) }
}

unsafe extern "system" fn get_playback(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("get_playback");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(Playback(current()), 0)) }
}

/// A `Try*Async` result.
unsafe fn done(out: *mut Ptr, ok: bool) -> Hresult {
    // SAFETY: out pointer from the caller.
    unsafe { put(out, completed(IID_ASYNC_BOOL, Res::Bool(ok))) }
}

macro_rules! try_command {
    ($name:ident, $cmd:expr, $need:expr) => {
        unsafe extern "system" fn $name(_: Ptr, out: *mut Ptr) -> Hresult {
            trace(stringify!($name));
            // SAFETY: out pointer from the caller.
            unsafe { done(out, command($cmd, $need)) }
        }
    };
}

try_command!(try_play, Command::Play, caps::PLAY);
try_command!(try_pause, Command::Pause, caps::PAUSE);
try_command!(try_stop, Command::Stop, caps::STOP);
try_command!(try_next, Command::Next, caps::NEXT);
try_command!(try_previous, Command::Previous, caps::PREVIOUS);
try_command!(try_toggle, Command::Toggle, caps::TOGGLE);

unsafe extern "system" fn try_unsupported(_: Ptr, out: *mut Ptr) -> Hresult {
    trace("try_unsupported");
    // SAFETY: out pointer from the caller.
    unsafe { done(out, false) }
}

unsafe extern "system" fn try_repeat(_: Ptr, mode: i32, out: *mut Ptr) -> Hresult {
    trace("try_repeat");
    let r = match mode {
        1 => Repeat::Track,
        2 => Repeat::List,
        _ => Repeat::None,
    };
    // SAFETY: out pointer from the caller.
    unsafe { done(out, command(Command::Repeat(r), caps::REPEAT)) }
}

unsafe extern "system" fn try_rate(_: Ptr, rate: f64, out: *mut Ptr) -> Hresult {
    trace("try_rate");
    // SAFETY: out pointer from the caller.
    unsafe { done(out, command(Command::Rate(rate), caps::RATE)) }
}

unsafe extern "system" fn try_shuffle(_: Ptr, on: bool, out: *mut Ptr) -> Hresult {
    trace("try_shuffle");
    // SAFETY: out pointer from the caller.
    unsafe { done(out, command(Command::Shuffle(on), caps::SHUFFLE)) }
}

unsafe extern "system" fn try_position(_: Ptr, ticks: i64, out: *mut Ptr) -> Hresult {
    trace("try_position");
    // SAFETY: out pointer from the caller.
    unsafe { done(out, command(Command::Seek(ticks / 10), caps::SEEK)) }
}

macro_rules! session_event {
    ($add:ident, $remove:ident, $field:ident) => {
        unsafe extern "system" fn $add(p: Ptr, h: Ptr, token: *mut i64) -> Hresult {
            // SAFETY: our interface pointer; caller's arguments.
            unsafe { this::<Session, 0>(p).val.$field.add(h, token) }
        }
        unsafe extern "system" fn $remove(p: Ptr, token: i64) -> Hresult {
            // SAFETY: our interface pointer.
            unsafe { this::<Session, 0>(p) }.val.$field.remove(token)
        }
    };
}

session_event!(add_timeline_changed, remove_timeline_changed, timeline);
session_event!(add_playback_changed, remove_playback_changed, playback);
session_event!(add_media_changed, remove_media_changed, media);

static SESSION_VT: VTable<31> = vtable!(Session, 0;
    source_app, try_get_properties, get_timeline, get_playback,
    try_play, try_pause, try_stop,
    try_unsupported, // TryRecordAsync
    try_unsupported, // TryFastForwardAsync
    try_unsupported, // TryRewindAsync
    try_next, try_previous,
    try_unsupported, // TryChangeChannelUpAsync
    try_unsupported, // TryChangeChannelDownAsync
    try_toggle, try_repeat, try_rate, try_shuffle, try_position,
    add_timeline_changed, remove_timeline_changed,
    add_playback_changed, remove_playback_changed,
    add_media_changed, remove_media_changed);

// ---------------------------------------------------------------- event args

/// The (empty) arguments of an event, of interface `iid`.
struct Args(Guid);

impl Class for Args {
    const NAME: &'static str = "Windows.Media.Control.EventArgs";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [ARGS_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == self.0).then_some(0)
    }
}

fn args(iid: Guid) -> Unk {
    Unk::own(create(Args(iid), 0)).expect("allocated")
}

static ARGS_VT: VTable<6> = vtable!(Args, 0;);

// ---------------------------------------------------------------- media properties

struct Properties(Arc<Snapshot>);

impl Class for Properties {
    const NAME: &'static str =
        "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [PROPERTIES_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_PROPERTIES).then_some(0)
    }
}

macro_rules! string_getter {
    ($t:ty, $name:ident, $field:ident) => {
        unsafe extern "system" fn $name(p: Ptr, out: *mut Hstring) -> Hresult {
            // SAFETY: our interface pointer; out pointer from the caller.
            unsafe { put(out, hstring(&wide(&this::<$t, 0>(p).val.0.$field))) }
        }
    };
}

string_getter!(Properties, title, title);
string_getter!(Properties, subtitle, subtitle);
string_getter!(Properties, album_artist, album_artist);
string_getter!(Properties, artist, artist);
string_getter!(Properties, album_title, album);

unsafe extern "system" fn track_number(p: Ptr, out: *mut i32) -> Hresult {
    trace("track_number");
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, this::<Properties, 0>(p).val.0.track) }
}

unsafe extern "system" fn album_track_count(p: Ptr, out: *mut i32) -> Hresult {
    trace("album_track_count");
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, this::<Properties, 0>(p).val.0.tracks) }
}

unsafe extern "system" fn genres(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("genres");
    // SAFETY: our interface pointer.
    let s = unsafe { this::<Properties, 0>(p) };
    let items = s.val.0.genres.iter().map(|g| Item::Str(wide(g))).collect();
    let v = View {
        kind: ViewKind::Strings,
        items: Arc::new(items),
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(v, 0)) }
}

fn kind_value(k: Kind) -> i32 {
    match k {
        Kind::Unknown => 0,
        Kind::Music => 1,
        Kind::Video => 2,
    }
}

unsafe extern "system" fn properties_kind(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("properties_kind");
    // SAFETY: our interface pointer.
    let k = kind_value(unsafe { this::<Properties, 0>(p) }.val.0.kind);
    // SAFETY: out pointer from the caller.
    unsafe { put(out, reference(IID_REF_KIND, Value::I32(k))) }
}

unsafe extern "system" fn thumbnail(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("thumbnail");
    // SAFETY: our interface pointer.
    let art = &unsafe { this::<Properties, 0>(p) }.val.0.art;
    let r = if art.is_empty() {
        null()
    } else {
        create(StreamReference(media_path(art)), 0)
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, r) }
}

static PROPERTIES_VT: VTable<16> = vtable!(Properties, 0;
    title, subtitle, album_artist, artist, album_title, track_number,
    genres, album_track_count, properties_kind, thumbnail);

// ---------------------------------------------------------------- playback info

struct Playback(Arc<Snapshot>);

impl Class for Playback {
    const NAME: &'static str =
        "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionPlaybackInfo";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [PLAYBACK_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_PLAYBACK).then_some(0)
    }
}

unsafe extern "system" fn playback_controls(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("playback_controls");
    // SAFETY: our interface pointer.
    let c = unsafe { this::<Playback, 0>(p) }.val.0.caps;
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(Controls(c), 0)) }
}

unsafe extern "system" fn playback_status(p: Ptr, out: *mut i32) -> Hresult {
    trace("playback_status");
    // SAFETY: our interface pointer.
    let st = match unsafe { this::<Playback, 0>(p) }.val.0.status {
        Status::None => 0, // Closed
        Status::Changing => 2,
        Status::Stopped => 3,
        Status::Playing => 4,
        Status::Paused => 5,
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, st) }
}

unsafe extern "system" fn playback_kind(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("playback_kind");
    // SAFETY: our interface pointer.
    let k = kind_value(unsafe { this::<Playback, 0>(p) }.val.0.kind);
    // SAFETY: out pointer from the caller.
    unsafe { put(out, reference(IID_REF_KIND, Value::I32(k))) }
}

unsafe extern "system" fn playback_repeat(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("playback_repeat");
    // SAFETY: our interface pointer.
    let r = unsafe { this::<Playback, 0>(p) }
        .val
        .0
        .repeat
        .map(|r| match r {
            Repeat::None => 0,
            Repeat::Track => 1,
            Repeat::List => 2,
        });
    let v = r.map_or_else(null, |r| reference(IID_REF_REPEAT, Value::I32(r)));
    // SAFETY: out pointer from the caller.
    unsafe { put(out, v) }
}

unsafe extern "system" fn playback_rate(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("playback_rate");
    // SAFETY: our interface pointer.
    let r = unsafe { this::<Playback, 0>(p) }.val.0.rate;
    // SAFETY: out pointer from the caller.
    unsafe { put(out, reference(IID_REF_F64, Value::F64(r))) }
}

unsafe extern "system" fn playback_shuffle(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("playback_shuffle");
    // SAFETY: our interface pointer.
    let s = unsafe { this::<Playback, 0>(p) }.val.0.shuffle;
    let v = s.map_or_else(null, |s| reference(IID_REF_BOOL, Value::Bool(s)));
    // SAFETY: out pointer from the caller.
    unsafe { put(out, v) }
}

static PLAYBACK_VT: VTable<12> = vtable!(Playback, 0;
    playback_controls, playback_status, playback_kind, playback_repeat,
    playback_rate, playback_shuffle);

// ---------------------------------------------------------------- playback controls

struct Controls(u16);

impl Class for Controls {
    const NAME: &'static str =
        "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionPlaybackControls";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [CONTROLS_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_CONTROLS).then_some(0)
    }
}

macro_rules! control {
    ($name:ident, $bits:expr) => {
        unsafe extern "system" fn $name(p: Ptr, out: *mut bool) -> Hresult {
            // SAFETY: our interface pointer; out pointer from the caller.
            unsafe { put(out, this::<Controls, 0>(p).val.0 & $bits != 0) }
        }
    };
}

control!(can_play, caps::PLAY);
control!(can_pause, caps::PAUSE);
control!(can_stop, caps::STOP);
control!(can_next, caps::NEXT);
control!(can_previous, caps::PREVIOUS);
control!(can_toggle, caps::TOGGLE);
control!(can_shuffle, caps::SHUFFLE);
control!(can_repeat, caps::REPEAT);
control!(can_rate, caps::RATE);
control!(can_seek, caps::SEEK);

unsafe extern "system" fn cannot(_: Ptr, out: *mut bool) -> Hresult {
    trace("cannot");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, false) }
}

static CONTROLS_VT: VTable<21> = vtable!(Controls, 0;
    can_play, can_pause, can_stop,
    cannot, // IsRecordEnabled
    cannot, // IsFastForwardEnabled
    cannot, // IsRewindEnabled
    can_next, can_previous,
    cannot, // IsChannelUpEnabled
    cannot, // IsChannelDownEnabled
    can_toggle, can_shuffle, can_repeat, can_rate, can_seek);

// ---------------------------------------------------------------- timeline

struct Timeline(Arc<Snapshot>);

impl Class for Timeline {
    const NAME: &'static str =
        "Windows.Media.Control.GlobalSystemMediaTransportControlsSessionTimelineProperties";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [TIMELINE_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_TIMELINE).then_some(0)
    }
}

unsafe extern "system" fn zero_time(_: Ptr, out: *mut i64) -> Hresult {
    trace("zero_time");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, 0) }
}

unsafe extern "system" fn end_time(p: Ptr, out: *mut i64) -> Hresult {
    trace("end_time");
    // SAFETY: our interface pointer; out pointer from the caller (100 ns).
    unsafe { put(out, this::<Timeline, 0>(p).val.0.length_us * 10) }
}

unsafe extern "system" fn position(p: Ptr, out: *mut i64) -> Hresult {
    trace("position");
    // SAFETY: our interface pointer; out pointer from the caller (100 ns).
    unsafe { put(out, this::<Timeline, 0>(p).val.0.position_us * 10) }
}

unsafe extern "system" fn last_updated(p: Ptr, out: *mut i64) -> Hresult {
    trace("last_updated");
    // SAFETY: our interface pointer.
    let ms = unsafe { this::<Timeline, 0>(p) }.val.0.updated_ms;
    // SAFETY: out pointer from the caller (DateTime: 100 ns since 1601).
    unsafe { put(out, ms * 10_000 + UNIX_EPOCH_TICKS) }
}

static TIMELINE_VT: VTable<12> = vtable!(Timeline, 0;
    zero_time, end_time, zero_time, end_time, position, last_updated);

// ---------------------------------------------------------------- IReference<T>

enum Value {
    I32(i32),
    F64(f64),
    Bool(bool),
}

struct Reference(Guid, Value);

impl Class for Reference {
    const NAME: &'static str = "Windows.Foundation.IReference`1";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [REFERENCE_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == self.0).then_some(0)
    }
}

fn reference(iid: Guid, v: Value) -> Ptr {
    create(Reference(iid, v), 0)
}

unsafe extern "system" fn reference_value(p: Ptr, out: *mut c_void) -> Hresult {
    trace("reference_value");
    // SAFETY: our interface pointer; out is a T* of the reference's type.
    unsafe {
        match this::<Reference, 0>(p).val.1 {
            Value::I32(v) => put(out.cast::<i32>(), v),
            Value::F64(v) => put(out.cast::<f64>(), v),
            Value::Bool(v) => put(out.cast::<bool>(), v),
        }
    }
}

static REFERENCE_VT: VTable<7> = vtable!(Reference, 0; reference_value);

// ---------------------------------------------------------------- IVectorView<T>

enum Item {
    Str(Vec<u16>),
    Obj(Unk),
}

#[derive(Clone, Copy)]
enum ViewKind {
    Sessions,
    Strings,
}

impl ViewKind {
    /// (IVectorView, IIterable, IIterator) of this kind.
    fn iids(self) -> (Guid, Guid, Guid) {
        match self {
            Self::Sessions => (
                IID_VIEW_SESSIONS,
                IID_ITERABLE_SESSIONS,
                IID_ITERATOR_SESSIONS,
            ),
            Self::Strings => (IID_VIEW_STRINGS, IID_ITERABLE_STRINGS, IID_ITERATOR_STRINGS),
        }
    }
}

/// A read-only list (and its IIterable).
struct View {
    kind: ViewKind,
    items: Arc<Vec<Item>>,
}

impl Class for View {
    const NAME: &'static str = "Windows.Foundation.Collections.IVectorView`1";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        let i = ITERABLE_VT.ptr();
        [VIEW_VT.ptr(), i, i, i, i, i]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        let (view, iterable, _) = self.kind.iids();
        if *iid == view {
            Some(0)
        } else if *iid == iterable {
            Some(1)
        } else {
            None
        }
    }
}

/// Writes item `i` (an HSTRING or an object, with a new reference).
unsafe fn put_item(items: &[Item], i: usize, out: *mut Ptr) -> Hresult {
    let v = match items.get(i) {
        Some(Item::Str(s)) => hstring(s),
        Some(Item::Obj(o)) => o.out(),
        None => return E_BOUNDS,
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, v) }
}

unsafe extern "system" fn view_get_at(p: Ptr, i: u32, out: *mut Ptr) -> Hresult {
    trace("view_get_at");
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put_item(&this::<View, 0>(p).val.items, i as usize, out) }
}

unsafe extern "system" fn view_size(p: Ptr, out: *mut u32) -> Hresult {
    trace("view_size");
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, this::<View, 0>(p).val.items.len() as u32) }
}

unsafe extern "system" fn view_index_of(
    _: Ptr,
    _: Ptr,
    index: *mut u32,
    found: *mut bool,
) -> Hresult {
    trace("view_index_of");
    // SAFETY: out pointers from the caller.
    unsafe {
        put(index, 0);
        put(found, false)
    }
}

/// Copies items `start..` into `out` (up to `cap`), counting them in `n`.
unsafe fn get_many(items: &[Item], start: usize, cap: u32, out: *mut Ptr, n: *mut u32) -> Hresult {
    let mut k = 0;
    while (k as u32) < cap && start + k < items.len() {
        // SAFETY: the caller's array holds cap items.
        unsafe { put_item(items, start + k, out.add(k)) };
        k += 1;
    }
    // SAFETY: out pointer from the caller.
    unsafe { put(n, k as u32) }
}

unsafe extern "system" fn view_get_many(
    p: Ptr,
    start: u32,
    cap: u32,
    out: *mut Ptr,
    n: *mut u32,
) -> Hresult {
    trace("view_get_many");
    // SAFETY: our interface pointer; the caller's array and count.
    unsafe { get_many(&this::<View, 0>(p).val.items, start as usize, cap, out, n) }
}

unsafe extern "system" fn view_first(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("view_first");
    // SAFETY: our interface pointer (slot 1).
    let v = unsafe { this::<View, 1>(p) };
    let it = Iter {
        iid: v.val.kind.iids().2,
        items: v.val.items.clone(),
        pos: Lock::new(0),
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(it, 0)) }
}

static VIEW_VT: VTable<10> = vtable!(View, 0; view_get_at, view_size, view_index_of, view_get_many);
static ITERABLE_VT: VTable<7> = vtable!(View, 1; view_first);

struct Iter {
    iid: Guid,
    items: Arc<Vec<Item>>,
    pos: Lock<usize>,
}

impl Class for Iter {
    const NAME: &'static str = "Windows.Foundation.Collections.IIterator`1";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [ITER_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == self.iid).then_some(0)
    }
}

unsafe extern "system" fn iter_current(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("iter_current");
    // SAFETY: our interface pointer.
    let it = unsafe { this::<Iter, 0>(p) };
    let i = it.val.pos.with(|p| *p);
    // SAFETY: out pointer from the caller.
    unsafe { put_item(&it.val.items, i, out) }
}

unsafe extern "system" fn iter_has_current(p: Ptr, out: *mut bool) -> Hresult {
    trace("iter_has_current");
    // SAFETY: our interface pointer.
    let it = unsafe { this::<Iter, 0>(p) };
    let i = it.val.pos.with(|p| *p);
    // SAFETY: out pointer from the caller.
    unsafe { put(out, i < it.val.items.len()) }
}

unsafe extern "system" fn iter_move_next(p: Ptr, out: *mut bool) -> Hresult {
    trace("iter_move_next");
    // SAFETY: our interface pointer.
    let it = unsafe { this::<Iter, 0>(p) };
    let i = it.val.pos.with(|p| {
        *p += 1;
        *p
    });
    // SAFETY: out pointer from the caller.
    unsafe { put(out, i < it.val.items.len()) }
}

unsafe extern "system" fn iter_get_many(p: Ptr, cap: u32, out: *mut Ptr, n: *mut u32) -> Hresult {
    trace("iter_get_many");
    // SAFETY: our interface pointer.
    let it = unsafe { this::<Iter, 0>(p) };
    let start = it.val.pos.with(|p| *p);
    // SAFETY: the caller's array and count.
    let hr = unsafe { get_many(&it.val.items, start, cap, out, n) };
    // SAFETY: checked by get_many.
    let taken = if n.is_null() { 0 } else { unsafe { *n } } as usize;
    it.val.pos.with(|p| *p += taken);
    hr
}

static ITER_VT: VTable<10> =
    vtable!(Iter, 0; iter_current, iter_has_current, iter_move_next, iter_get_many);

// ---------------------------------------------------------------- album art

/// The album art's `IRandomAccessStreamReference` (a PNG file the engine
/// wrote, read when opened).
struct StreamReference(Vec<u16>);

impl Class for StreamReference {
    const NAME: &'static str = "Windows.Storage.Streams.RandomAccessStreamReference";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [STREAM_REFERENCE_VT.ptr(); SLOTS]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        (*iid == IID_STREAM_REFERENCE).then_some(0)
    }
}

unsafe extern "system" fn open_read(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("open_read");
    // SAFETY: our interface pointer.
    let path = &unsafe { this::<StreamReference, 0>(p) }.val.0;
    let data = read_file(path).unwrap_or_else(|| {
        log("album art missing");
        Vec::new()
    });
    let s = Unk::own(create(
        Stream {
            data: Arc::new(data),
            pos: Lock::new(0),
        },
        0,
    ));
    // SAFETY: out pointer from the caller.
    unsafe { put(out, completed(IID_ASYNC_STREAM, Res::Obj(s))) }
}

static STREAM_REFERENCE_VT: VTable<7> = vtable!(StreamReference, 0; open_read);

/// A read-only in-memory stream: IRandomAccessStreamWithContentType (0),
/// IRandomAccessStream (1), IInputStream (2), IOutputStream (3),
/// IClosable (4), IContentTypeProvider (5).
struct Stream {
    data: Arc<Vec<u8>>,
    pos: Lock<u64>,
}

impl Class for Stream {
    const NAME: &'static str = "Windows.Storage.Streams.RandomAccessStream";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        [
            STREAM_WITH_TYPE_VT.ptr(),
            RANDOM_ACCESS_VT.ptr(),
            INPUT_VT.ptr(),
            OUTPUT_VT.ptr(),
            CLOSABLE_VT.ptr(),
            CONTENT_TYPE_VT.ptr(),
        ]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        [
            IID_STREAM_WITH_TYPE,
            IID_RANDOM_ACCESS_STREAM,
            IID_INPUT_STREAM,
            IID_OUTPUT_STREAM,
            IID_CLOSABLE,
            IID_CONTENT_TYPE,
        ]
        .iter()
        .position(|i| i == iid)
        .or((*iid == crate::datareader::IID_OUR_BYTES).then_some(0))
    }
}

/// The bytes and position of our stream, from its interface
/// `datareader::IID_OUR_BYTES` (slot 0).
pub fn stream_bytes(p: Ptr) -> (Arc<Vec<u8>>, usize) {
    // SAFETY: only our Stream answers that interface, with slot 0.
    let s = unsafe { this::<Stream, 0>(p) };
    let pos = s.val.pos.with(|p| *p) as usize;
    (s.val.data.clone(), pos.min(s.val.data.len()))
}

/// Another stream over the same bytes, at `pos`, as slot `slot`.
fn stream_at(data: &Arc<Vec<u8>>, pos: u64, slot: usize) -> Ptr {
    create(
        Stream {
            data: data.clone(),
            pos: Lock::new(pos),
        },
        slot,
    )
}

unsafe extern "system" fn stream_size(p: Ptr, out: *mut u64) -> Hresult {
    trace("stream_size");
    // SAFETY: our interface pointer (slot 1); out pointer from the caller.
    unsafe { put(out, this::<Stream, 1>(p).val.data.len() as u64) }
}

unsafe extern "system" fn stream_input_at(p: Ptr, pos: u64, out: *mut Ptr) -> Hresult {
    trace("stream_input_at");
    // SAFETY: our interface pointer (slot 1); out pointer from the caller.
    unsafe { put(out, stream_at(&this::<Stream, 1>(p).val.data, pos, 2)) }
}

unsafe extern "system" fn stream_position(p: Ptr, out: *mut u64) -> Hresult {
    trace("stream_position");
    // SAFETY: our interface pointer (slot 1).
    let pos = unsafe { this::<Stream, 1>(p) }.val.pos.with(|p| *p);
    // SAFETY: out pointer from the caller.
    unsafe { put(out, pos) }
}

unsafe extern "system" fn stream_seek(p: Ptr, pos: u64) -> Hresult {
    trace("stream_seek");
    // SAFETY: our interface pointer (slot 1).
    unsafe { this::<Stream, 1>(p) }.val.pos.with(|p| *p = pos);
    S_OK
}

unsafe extern "system" fn stream_clone(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("stream_clone");
    // SAFETY: our interface pointer (slot 1); out pointer from the caller.
    unsafe { put(out, stream_at(&this::<Stream, 1>(p).val.data, 0, 1)) }
}

unsafe extern "system" fn yes(_: Ptr, out: *mut bool) -> Hresult {
    trace("yes");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, true) }
}

/// `ReadAsync(buffer, count, options)`: copies up to `count` bytes from the
/// position into the caller's buffer (through IBufferByteAccess), sets its
/// length, and gives the buffer back.
unsafe extern "system" fn stream_read(
    p: Ptr,
    buffer: Ptr,
    count: u32,
    _options: u32,
    out: *mut Ptr,
) -> Hresult {
    trace("stream_read");
    // SAFETY: our interface pointer (slot 2).
    let s = unsafe { this::<Stream, 2>(p) };
    let Some(buf) = Unk::borrow(buffer) else {
        return E_POINTER;
    };
    let Some(bytes) = buf.cast(&IID_IBUFFERBYTEACCESS) else {
        log("ReadAsync: the buffer has no IBufferByteAccess");
        return E_NOINTERFACE;
    };
    let mut cap = 0u32;
    let mut dst: *mut u8 = core::ptr::null_mut();
    // SAFETY: IBuffer::get_Capacity (slot 6) and IBufferByteAccess::Buffer
    // (slot 3) on the caller's buffer.
    unsafe {
        vcall::<unsafe extern "system" fn(Ptr, *mut u32) -> Hresult>(buf.ptr(), 6)(
            buf.ptr(),
            &mut cap,
        );
        vcall::<unsafe extern "system" fn(Ptr, *mut *mut u8) -> Hresult>(bytes.ptr(), 3)(
            bytes.ptr(),
            &mut dst,
        );
    }
    if dst.is_null() {
        return E_POINTER;
    }
    let n = s.val.pos.with(|pos| {
        let start = (*pos as usize).min(s.val.data.len());
        let n = (s.val.data.len() - start)
            .min(count as usize)
            .min(cap as usize);
        // SAFETY: dst holds cap bytes; n <= cap and within the data.
        unsafe { core::ptr::copy_nonoverlapping(s.val.data.as_ptr().add(start), dst, n) };
        *pos += n as u64;
        n
    });
    // SAFETY: IBuffer::put_Length (slot 8).
    unsafe {
        vcall::<unsafe extern "system" fn(Ptr, u32) -> Hresult>(buf.ptr(), 8)(buf.ptr(), n as u32)
    };
    // SAFETY: out pointer from the caller.
    unsafe {
        put(
            out,
            completed_with_progress(IID_ASYNC_READ, Res::Obj(Some(buf))),
        )
    }
}

unsafe extern "system" fn closed(_: Ptr) -> Hresult {
    trace("closed");
    S_OK
}

unsafe extern "system" fn content_type(_: Ptr, out: *mut Hstring) -> Hresult {
    trace("content_type");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, hstring(&wide("image/png"))) }
}

static STREAM_WITH_TYPE_VT: VTable<6> = vtable!(Stream, 0;);
static RANDOM_ACCESS_VT: VTable<15> = vtable!(Stream, 1;
    stream_size, notimpl!("IRandomAccessStream::put_Size"), stream_input_at,
    notimpl!("IRandomAccessStream::GetOutputStreamAt"), stream_position, stream_seek,
    stream_clone, yes, cannot);
static INPUT_VT: VTable<7> = vtable!(Stream, 2; stream_read);
static OUTPUT_VT: VTable<8> = vtable!(Stream, 3;
    notimpl!("IOutputStream::WriteAsync"), notimpl!("IOutputStream::FlushAsync"));
static CLOSABLE_VT: VTable<7> = vtable!(Stream, 4; closed);
static CONTENT_TYPE_VT: VTable<7> = vtable!(Stream, 5; content_type);
