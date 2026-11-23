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

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        // SAFETY: ends this process.
        unsafe { ExitProcess(1) }
    }

    // Without a C runtime, the few symbols the compiler and core expect.
    // Byte loops through volatile accesses, so they are not turned back into
    // calls to themselves.

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memset(d: *mut c_void, c: i32, n: usize) -> *mut c_void {
        let p = d.cast::<u8>();
        for i in 0..n {
            // SAFETY: the caller passes n writable bytes at d.
            unsafe { core::ptr::write_volatile(p.add(i), c as u8) };
        }
        d
    }

    /// Copies n bytes, front to back or back to front so overlapping
    /// ranges work (memmove; memcpy is the same).
    unsafe fn copy(d: *mut c_void, s: *const c_void, n: usize) -> *mut c_void {
        let (dp, sp) = (d.cast::<u8>(), s.cast::<u8>());
        let step = |i: usize| {
            // SAFETY: the caller passes n bytes at s and n writable at d.
            unsafe { core::ptr::write_volatile(dp.add(i), core::ptr::read_volatile(sp.add(i))) }
        };
        if (d as usize) < (s as usize) {
            (0..n).for_each(step);
        } else {
            (0..n).rev().for_each(step);
        }
        d
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcpy(d: *mut c_void, s: *const c_void, n: usize) -> *mut c_void {
        // SAFETY: forwards the caller's guarantee.
        unsafe { copy(d, s, n) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memmove(d: *mut c_void, s: *const c_void, n: usize) -> *mut c_void {
        // SAFETY: forwards the caller's guarantee.
        unsafe { copy(d, s, n) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
        let (a, b) = (a.cast::<u8>(), b.cast::<u8>());
        for i in 0..n {
            // SAFETY: the caller passes n readable bytes at a and b.
            let (x, y) = unsafe {
                (
                    core::ptr::read_volatile(a.add(i)),
                    core::ptr::read_volatile(b.add(i)),
                )
            };
            if x != y {
                return i32::from(x) - i32::from(y);
            }
        }
        0
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, n: usize) -> i32 {
        // SAFETY: forwards the caller's guarantee.
        unsafe { memcmp(a, b, n) }
    }

    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn wcslen(s: *const u16) -> usize {
        let mut n = 0;
        // SAFETY: the caller passes a NUL-terminated string.
        while unsafe { core::ptr::read_volatile(s.add(n)) } != 0 {
            n += 1;
        }
        n
    }

    /// core is built with unwinding; with `panic = abort` nothing unwinds,
    /// but its tables still name the MSVC personality routine.
    #[unsafe(no_mangle)]
    pub extern "C" fn __CxxFrameHandler3() -> i32 {
        1
    }

    #[unsafe(no_mangle)]
    pub static _fltused: i32 = 0;
}
