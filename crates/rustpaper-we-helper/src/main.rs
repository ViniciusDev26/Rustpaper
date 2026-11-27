//! `rustpaper-we-helper.exe`: runs inside Wallpaper Engine's Proton prefix
//! (the engine starts it with `wine`, in WE's container, like the
//! `-control` commands: we.rs) and marks Rustpaper's WE windows so web
//! wallpapers take clicks.
//!
//! Why: WE draws a web wallpaper with an embedded Chromium in child windows
//! of the WE window (`WPEDesktopCEFWindow` > `Chrome_WidgetWin_1` >
//! `Chrome_RenderWidgetHostHWND`). Its `webwallpaper64.exe` polls the
//! cursor and the left button itself and forwards a click only when
//! `WindowFromPoint` is the Windows desktop (class `SysListView32`,
//! `SHELLDLL_DefView` or `Progman`) or carries the window property
//! `WallpaperEngineParent` = 1 (read from its disassembly, WE of
//! 2026-11-08). Wine has no such desktop windows and WE does not set the
//! property on the windows of `-playInWindow`, so hover reached the page and
//! clicks never did. With the property set on the WE window and all its
//! children, the page took clicks (2026-11-23).
//!
//! WE recreates the Chromium windows with every wallpaper, so this keeps
//! watching: every 500 ms it marks the windows titled `rustpaper-*` and
//! their descendants. It exits on its own once no such window is left for
//! a few seconds (WE quit), so it never holds WE's Wine prefix: Steam
//! refuses to start WE again while processes of its last launch remain.

#![cfg_attr(windows, no_std, no_main)]

#[cfg(not(windows))]
fn main() {
    eprintln!("rustpaper-we-helper is a Windows program: it runs inside Wallpaper Engine's Proton");
    std::process::exit(2);
}

#[cfg(windows)]
mod rt;

#[cfg(windows)]
mod win {
    use core::ffi::c_void;
    use core::sync::atomic::{AtomicUsize, Ordering};

    type Hwnd = *mut c_void;
    type EnumProc = extern "system" fn(Hwnd, isize) -> i32;

    #[link(name = "user32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn EnumWindows(f: EnumProc, l: isize) -> i32;
        fn EnumChildWindows(parent: Hwnd, f: EnumProc, l: isize) -> i32;
        fn GetWindowTextW(h: Hwnd, s: *mut u16, n: i32) -> i32;
        fn GetPropW(h: Hwnd, key: *const u16) -> *mut c_void;
        fn SetPropW(h: Hwnd, key: *const u16, data: *mut c_void) -> i32;
    }

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn Sleep(ms: u32);
        fn ExitProcess(code: u32) -> !;
    }

    /// `s` as a NUL-terminated UTF-16 array (ASCII only).
    const fn wide<const N: usize>(s: &[u8; N]) -> [u16; N] {
        let mut w = [0u16; N];
        let mut i = 0;
        while i < N {
            w[i] = s[i] as u16;
            i += 1;
        }
        w
    }

    static KEY: [u16; 22] = wide(b"WallpaperEngineParent\0");
    static PREFIX: [u16; 10] = wide(b"rustpaper-");

    /// `rustpaper-*` windows seen in the current pass.
    static FOUND: AtomicUsize = AtomicUsize::new(0);

    const POLL_MS: u32 = 500;
    /// Passes without any `rustpaper-*` window before exiting (5 s).
    const IDLE_PASSES: u32 = 10;

    fn mark(h: Hwnd) {
        // SAFETY: plain user32 calls on a handle Windows just enumerated; a
        // window gone meanwhile only makes them fail.
        unsafe {
            if GetPropW(h, KEY.as_ptr()) as usize != 1 {
                // the property is a plain value (1), not a pointer
                SetPropW(h, KEY.as_ptr(), core::ptr::without_provenance_mut(1));
            }
        }
    }

    extern "system" fn child(h: Hwnd, _: isize) -> i32 {
        mark(h);
        1
    }

    extern "system" fn top(h: Hwnd, _: isize) -> i32 {
        let mut title = [0u16; 64];
        // SAFETY: the buffer holds 64 units and its length is passed.
        let n = unsafe { GetWindowTextW(h, title.as_mut_ptr(), title.len() as i32) };
        if n as usize > PREFIX.len() && title[..PREFIX.len()] == PREFIX {
            FOUND.fetch_add(1, Ordering::Relaxed);
            mark(h);
            // every descendant, not only direct children
            // SAFETY: enumerating a handle Windows just gave us.
            unsafe { EnumChildWindows(h, child, 0) };
        }
        1
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn mainCRTStartup() -> ! {
        let mut idle = 0;
        loop {
            FOUND.store(0, Ordering::Relaxed);
            // SAFETY: EnumWindows with a callback that only reads windows.
            unsafe { EnumWindows(top, 0) };
            idle = if FOUND.load(Ordering::Relaxed) == 0 {
                idle + 1
            } else {
                0
            };
            if idle >= IDLE_PASSES {
                // SAFETY: ends this process.
                unsafe { ExitProcess(0) };
            }
            // SAFETY: plain sleep.
            unsafe { Sleep(POLL_MS) };
        }
    }
}
