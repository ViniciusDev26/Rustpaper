//! What the compiler and `core` expect from a C runtime, for the Windows
//! artifacts of this crate (the helper .exe and the media DLL), which link
//! none (`/NODEFAULTLIB`, .cargo/config.toml).

use core::ffi::c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
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
pub unsafe extern "C" fn strlen(s: *const core::ffi::c_char) -> usize {
    let mut n = 0;
    // SAFETY: the caller passes a NUL-terminated string.
    while unsafe { core::ptr::read_volatile(s.add(n)) } != 0 {
        n += 1;
    }
    n
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
