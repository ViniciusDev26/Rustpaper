//! `rustpaper_media.dll`: Windows' media session API
//! (`Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`)
//! for Wallpaper Engine under Proton, fed with what plays on the host.
//!
//! Why: WE's media integration (album art, title, artist and playback
//! state for scenes: `mediaThumbnailChanged`, `mediaPropertiesChanged`...)
//! runs in WE's helper `bin/winrtutil64.exe -messagehandler
//! -mediainterface -parentprocess <WE>`, which asks the Windows Runtime for
//! that class. Proton's Wine has none: the activation failed
//! (`REGDB_E_CLASSNOTREG`, then `CLASS_E_CLASSNOTAVAILABLE` from Wine's
//! windows.media.dll), the helper died on the unhandled C++/WinRT exception
//! (c0000409), and scenes never got a media event: a layer showing the
//! album art stayed visible with an empty, white texture (traced with
//! `WINEDEBUG=+relay`, 2026-11-27).
//!
//! The engine (`media.rs` there) registers this DLL for that class in WE's
//! prefix (`HKLM\Software\Microsoft\WindowsRuntime\ActivatableClassId`,
//! `DllPath`) and writes what the host's MPRIS players play into the media
//! folder next to it (`format.rs`); WE's own helper then runs unchanged.
//! The classes are in `gsmtc.rs`, the COM plumbing in `com.rs`. The album
//! art also needs `Windows.Storage.Streams.DataReader` over a stream, a
//! stub in Wine: `datareader.rs` provides it for our stream and hands the
//! rest to Wine's.
//!
//! On the host only [`format`] is built (shared with the engine).

#![cfg_attr(windows, no_std)]

extern crate alloc;

pub mod format;

#[cfg(windows)]
mod com;
#[cfg(windows)]
mod datareader;
#[cfg(windows)]
mod gsmtc;
#[cfg(windows)]
mod rt;
#[cfg(windows)]
mod sys;
