//! The DLL's few Windows imports (kernel32, through raw-dylib: no SDK), its
//! heap allocator, a lock, files, and a log.

use alloc::string::String;
use alloc::vec::Vec;
use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ffi::c_void;

pub type Handle = *mut c_void;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetProcessHeap() -> Handle;
    fn HeapAlloc(heap: Handle, flags: u32, n: usize) -> *mut c_void;
    fn HeapReAlloc(heap: Handle, flags: u32, p: *mut c_void, n: usize) -> *mut c_void;
    fn HeapFree(heap: Handle, flags: u32, p: *mut c_void) -> i32;
    fn AcquireSRWLockExclusive(lock: *mut *mut c_void);
    fn ReleaseSRWLockExclusive(lock: *mut *mut c_void);
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn ReadFile(h: Handle, buf: *mut c_void, n: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn WriteFile(h: Handle, buf: *const c_void, n: u32, written: *mut u32, ov: *mut c_void) -> i32;
    fn GetFileSizeEx(h: Handle, size: *mut i64) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetFileAttributesExW(name: *const u16, level: i32, info: *mut FileAttributes) -> i32;
    fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut Handle) -> i32;
    fn GetModuleFileNameW(module: Handle, buf: *mut u16, n: u32) -> u32;
    fn CreateThread(
        security: *mut c_void,
        stack: usize,
        start: unsafe extern "system" fn(*mut c_void) -> u32,
        arg: *mut c_void,
        flags: u32,
        id: *mut u32,
    ) -> Handle;
    pub fn Sleep(ms: u32);
    fn GetCurrentProcessId() -> u32;
    fn GetTickCount64() -> u64;
}

const INVALID_HANDLE: Handle = usize::MAX as Handle;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_APPEND_DATA: u32 = 4;
const SHARE_ALL: u32 = 1 | 2 | 4;
const CREATE_ALWAYS: u32 = 2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const MOVEFILE_REPLACE_EXISTING: u32 = 1;
const HEAP_ZERO_MEMORY: u32 = 8;

/// `WIN32_FILE_ATTRIBUTE_DATA` (FILETIMEs as their two DWORDs).
#[repr(C)]
#[derive(Default)]
struct FileAttributes {
    attributes: u32,
    created: [u32; 2],
    accessed: [u32; 2],
    written: [u32; 2],
    size_high: u32,
    size_low: u32,
}

/// The process heap. Its blocks are 16-byte aligned on x64, which covers
/// every type here.
struct Heap;

// SAFETY: HeapAlloc & co. are thread-safe and return blocks of at least
// the requested size, 16-byte aligned; larger alignments are refused.
unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if l.align() > 16 {
            return core::ptr::null_mut();
        }
        // SAFETY: plain heap call.
        unsafe { HeapAlloc(GetProcessHeap(), 0, l.size()).cast() }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        if l.align() > 16 {
            return core::ptr::null_mut();
        }
        // SAFETY: plain heap call.
        unsafe { HeapAlloc(GetProcessHeap(), HEAP_ZERO_MEMORY, l.size()).cast() }
    }
    unsafe fn dealloc(&self, p: *mut u8, _: Layout) {
        // SAFETY: p came from this heap.
        unsafe { HeapFree(GetProcessHeap(), 0, p.cast()) };
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if l.align() > 16 {
            return core::ptr::null_mut();
        }
        // SAFETY: p came from this heap.
        unsafe { HeapReAlloc(GetProcessHeap(), 0, p.cast(), n).cast() }
    }
}

#[global_allocator]
static HEAP: Heap = Heap;

/// A value behind a Windows slim reader/writer lock.
pub struct Lock<T> {
    lock: UnsafeCell<*mut c_void>,
    value: UnsafeCell<T>,
}

// SAFETY: the value is only reached with the lock held.
unsafe impl<T: Send> Sync for Lock<T> {}
// SAFETY: as above.
unsafe impl<T: Send> Send for Lock<T> {}

impl<T> Lock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            lock: UnsafeCell::new(core::ptr::null_mut()),
            value: UnsafeCell::new(value),
        }
    }

    /// Runs `f` with the value, the lock held (never call back into code
    /// that takes the same lock: SRW locks are not recursive).
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // SAFETY: an SRWLOCK is a pointer-sized value initialized to 0; the
        // value is only borrowed while it is held.
        unsafe {
            AcquireSRWLockExclusive(self.lock.get());
            let r = f(&mut *self.value.get());
            ReleaseSRWLockExclusive(self.lock.get());
            r
        }
    }
}

/// The folder this DLL is in, with a trailing backslash (UTF-16, no NUL).
fn own_dir() -> Vec<u16> {
    let mut module: Handle = core::ptr::null_mut();
    let mut buf = alloc::vec![0u16; 1024];
    // SAFETY: FROM_ADDRESS (4) | UNCHANGED_REFCOUNT (2) with an address in
    // this DLL; the buffer's length is passed.
    let n = unsafe {
        GetModuleHandleExW(6, own_dir as *const c_void, &mut module);
        GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32)
    } as usize;
    buf.truncate(n);
    let cut = buf
        .iter()
        .rposition(|&c| c == u16::from(b'\\'))
        .map_or(0, |i| i + 1);
    buf.truncate(cut);
    buf
}

/// The media folder (format.rs): `media\` next to this DLL.
pub fn media_dir() -> &'static [u16] {
    static DIR: Lock<Option<&'static [u16]>> = Lock::new(None);
    DIR.with(|d| {
        *d.get_or_insert_with(|| {
            let mut dir = own_dir();
            dir.extend("media\\".encode_utf16());
            alloc::boxed::Box::leak(dir.into_boxed_slice())
        })
    })
}

/// `name` in the media folder, NUL-terminated.
pub fn media_path(name: &str) -> Vec<u16> {
    let mut p = media_dir().to_vec();
    p.extend(name.encode_utf16());
    p.push(0);
    p
}

/// A whole file; None when it cannot be read.
pub fn read_file(path: &[u16]) -> Option<Vec<u8>> {
    // SAFETY: path is NUL-terminated; the handle is closed below.
    unsafe {
        let h = CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_EXISTING,
            0,
            core::ptr::null_mut(),
        );
        if h == INVALID_HANDLE {
            return None;
        }
        let mut size = 0i64;
        let mut data = Vec::new();
        let ok = GetFileSizeEx(h, &mut size) != 0 && (0..64 << 20).contains(&size);
        if ok {
            data.resize(size as usize, 0);
            let mut done = 0usize;
            while done < data.len() {
                let mut n = 0u32;
                let want = (data.len() - done).min(1 << 20) as u32;
                if ReadFile(
                    h,
                    data.as_mut_ptr().add(done).cast(),
                    want,
                    &mut n,
                    core::ptr::null_mut(),
                ) == 0
                    || n == 0
                {
                    break;
                }
                done += n as usize;
            }
            data.truncate(done);
        }
        CloseHandle(h);
        ok.then_some(data)
    }
}

/// Writes `data` to `name` in the media folder: aside first, then renamed
/// over it, so the engine never reads half a file.
pub fn write_media_file(name: &str, data: &[u8]) -> bool {
    let tmp = media_path(&alloc::format!(".{name}.tmp"));
    let dst = media_path(name);
    // SAFETY: NUL-terminated paths; the handle is closed below.
    unsafe {
        let h = CreateFileW(
            tmp.as_ptr(),
            GENERIC_WRITE,
            SHARE_ALL,
            core::ptr::null_mut(),
            CREATE_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        if h == INVALID_HANDLE {
            return false;
        }
        let mut n = 0u32;
        let ok = WriteFile(
            h,
            data.as_ptr().cast(),
            data.len() as u32,
            &mut n,
            core::ptr::null_mut(),
        ) != 0
            && n as usize == data.len();
        CloseHandle(h);
        ok && MoveFileExW(tmp.as_ptr(), dst.as_ptr(), MOVEFILE_REPLACE_EXISTING) != 0
    }
}

/// A file's last write time and size (to notice a change cheaply); None
/// when it is missing.
pub fn stamp(path: &[u16]) -> Option<(u32, u32, u32)> {
    let mut a = FileAttributes::default();
    // SAFETY: NUL-terminated path; level 0 is GetFileExInfoStandard.
    (unsafe { GetFileAttributesExW(path.as_ptr(), 0, &mut a) } != 0).then_some((
        a.written[0],
        a.written[1],
        a.size_low,
    ))
}

/// Starts `f` on a new thread.
pub fn spawn(f: unsafe extern "system" fn(*mut c_void) -> u32) {
    // SAFETY: a plain thread with no argument; its handle is closed.
    unsafe {
        let h = CreateThread(
            core::ptr::null_mut(),
            0,
            f,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
        );
        if !h.is_null() {
            CloseHandle(h);
        }
    }
}

/// Milliseconds since boot.
pub fn ticks() -> u64 {
    // SAFETY: plain call.
    unsafe { GetTickCount64() }
}

/// Whether `media\trace` exists (checked once): then [`trace`] logs every
/// call into the classes, to see what WE asks for.
pub fn tracing() -> bool {
    static ON: Lock<Option<bool>> = Lock::new(None);
    ON.with(|on| *on.get_or_insert_with(|| stamp(&media_path("trace")).is_some()))
}

/// Logs `what` when [`tracing`].
pub fn trace(what: &str) {
    if tracing() {
        log(what);
    }
}

/// Appends a line to `media\log.txt` (kept under 256 kB): what WE asked
/// that this DLL does not provide, and the like. Without a console or a
/// debugger under Proton, this is how to see it.
pub fn log(msg: &str) {
    let path = media_path("log.txt");
    if stamp(&path).is_some_and(|(_, _, size)| size > 256 << 10) {
        return;
    }
    // SAFETY: plain call.
    let pid = unsafe { GetCurrentProcessId() };
    let mut line = String::new();
    let _ = core::fmt::Write::write_fmt(&mut line, format_args!("[{pid} {}] {msg}\r\n", ticks()));
    // SAFETY: NUL-terminated path; the handle is closed below.
    unsafe {
        let h = CreateFileW(
            path.as_ptr(),
            FILE_APPEND_DATA,
            SHARE_ALL,
            core::ptr::null_mut(),
            OPEN_ALWAYS,
            0,
            core::ptr::null_mut(),
        );
        if h != INVALID_HANDLE {
            let mut n = 0u32;
            WriteFile(
                h,
                line.as_ptr().cast(),
                line.len() as u32,
                &mut n,
                core::ptr::null_mut(),
            );
            CloseHandle(h);
        }
    }
}
