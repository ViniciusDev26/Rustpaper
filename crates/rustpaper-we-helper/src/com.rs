//! Just enough COM / Windows Runtime to implement classes by hand: objects
//! with several interfaces (one vtable pointer each), generic IInspectable
//! methods, references to foreign objects, events, and asynchronous
//! operations that are complete from the start.
//!
//! An object is an [`Obj`]: an array of vtable pointers followed by its
//! reference count and value. An interface pointer is the address of one
//! slot of that array; a method recovers the object from it with the slot's
//! index, which every method knows at compile time (`const I`).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::sys::{Lock, log, trace};

pub type Hresult = i32;
pub type Hstring = *mut c_void;
pub type Ptr = *mut c_void;

pub const S_OK: Hresult = 0;
pub const S_FALSE: Hresult = 1;
pub const E_NOTIMPL: Hresult = 0x8000_4001_u32 as i32;
pub const E_NOINTERFACE: Hresult = 0x8000_4002_u32 as i32;
pub const E_POINTER: Hresult = 0x8000_4003_u32 as i32;
pub const E_BOUNDS: Hresult = 0x8000_000B_u32 as i32;
pub const E_INVALIDARG: Hresult = 0x8007_0057_u32 as i32;
pub const CLASS_E_CLASSNOTAVAILABLE: Hresult = 0x8004_0111_u32 as i32;

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

impl Guid {
    /// From the usual hex form written as one number
    /// (`0x00000000_0000_0000_c000_000000000046`).
    pub const fn new(u: u128) -> Self {
        let b = u.to_be_bytes();
        Self {
            d1: u32::from_be_bytes([b[0], b[1], b[2], b[3]]),
            d2: u16::from_be_bytes([b[4], b[5]]),
            d3: u16::from_be_bytes([b[6], b[7]]),
            d4: [b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]],
        }
    }
}

impl core::fmt::Display for Guid {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let d = &self.d4;
        write!(
            f,
            "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
            self.d1, self.d2, self.d3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
        )
    }
}

pub const IID_IUNKNOWN: Guid = Guid::new(0x00000000_0000_0000_c000_000000000046);
pub const IID_IINSPECTABLE: Guid = Guid::new(0xaf86e2e0_b12d_4c6a_9c5a_d7aa65101e90);
pub const IID_IAGILEOBJECT: Guid = Guid::new(0x94ea2b94_e9cc_49e0_c0ff_ee64ca8f5b90);
pub const IID_IASYNCINFO: Guid = Guid::new(0x00000036_0000_0000_c000_000000000046);
pub const IID_IBUFFERBYTEACCESS: Guid = Guid::new(0x905a0fef_bc53_11df_8c49_001e4fc686da);

#[link(name = "combase", kind = "raw-dylib")]
unsafe extern "system" {
    fn WindowsCreateString(s: *const u16, n: u32, out: *mut Hstring) -> Hresult;
    fn WindowsGetStringRawBuffer(h: Hstring, n: *mut u32) -> *const u16;
}

/// A new HSTRING (the caller owns it); null is the empty string.
pub fn hstring(s: &[u16]) -> Hstring {
    let mut h = core::ptr::null_mut();
    // SAFETY: s is a valid UTF-16 buffer of that length.
    unsafe { WindowsCreateString(s.as_ptr(), s.len() as u32, &mut h) };
    h
}

/// An HSTRING's characters.
///
/// # Safety
/// `h` is a valid HSTRING (or null), alive while the slice is used.
pub unsafe fn hstring_chars<'a>(h: Hstring) -> &'a [u16] {
    let mut n = 0;
    // SAFETY: per the caller.
    unsafe {
        let p = WindowsGetStringRawBuffer(h, &mut n);
        if p.is_null() || n == 0 {
            &[]
        } else {
            core::slice::from_raw_parts(p, n as usize)
        }
    }
}

/// Writes `v` through an out pointer, E_POINTER when it is null.
///
/// # Safety
/// `out` is null or valid for a write.
pub unsafe fn put<T>(out: *mut T, v: T) -> Hresult {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: per the caller.
    unsafe { out.write(v) };
    S_OK
}

/// Vtable pointers per object, at most.
pub const SLOTS: usize = 6;

/// A vtable: function pointers as raw pointers (a static needs `Sync`).
#[repr(transparent)]
pub struct VTable<const N: usize>(pub [*const c_void; N]);
// SAFETY: function pointers only, never written.
unsafe impl<const N: usize> Sync for VTable<N> {}

impl<const N: usize> VTable<N> {
    pub const fn ptr(&'static self) -> *const c_void {
        (self as *const Self).cast()
    }
}

/// A class implemented here.
pub trait Class: Sized + Send + Sync + 'static {
    /// Its runtime class name (GetRuntimeClassName).
    const NAME: &'static str;
    /// Its vtables, in slot order.
    fn vtables(&self) -> [*const c_void; SLOTS];
    /// The slot of the interface `iid`, besides IUnknown, IInspectable and
    /// IAgileObject (slot 0).
    fn slot(&self, iid: &Guid) -> Option<usize>;
}

#[repr(C)]
pub struct Obj<T> {
    vt: [*const c_void; SLOTS],
    refs: AtomicU32,
    pub val: T,
}

/// A new object (one reference), as its interface in slot `slot`.
pub fn create<T: Class>(val: T, slot: usize) -> Ptr {
    let vt = val.vtables();
    let o = Box::into_raw(Box::new(Obj {
        vt,
        refs: AtomicU32::new(1),
        val,
    }));
    // SAFETY: o is a fresh, valid object.
    unsafe { (&raw mut (*o).vt[slot]).cast() }
}

/// The object behind interface pointer `p` of slot `I`.
///
/// # Safety
/// `p` is slot `I` of a live `Obj<T>`.
pub unsafe fn this<'a, T, const I: usize>(p: Ptr) -> &'a Obj<T> {
    // SAFETY: per the caller, p points at vt[I].
    unsafe { &*p.cast::<*const c_void>().sub(I).cast::<Obj<T>>() }
}

impl<T> Obj<T> {
    /// Interface pointer of slot `slot`, with a new reference.
    pub fn iface(&self, slot: usize) -> Ptr {
        self.refs.fetch_add(1, Ordering::Relaxed);
        (&raw const self.vt[slot]).cast_mut().cast()
    }
}

pub unsafe extern "system" fn qi<T: Class, const I: usize>(
    p: Ptr,
    iid: *const Guid,
    out: *mut Ptr,
) -> Hresult {
    if out.is_null() || iid.is_null() {
        return E_POINTER;
    }
    // SAFETY: COM calls this on our interface pointer.
    let (o, iid) = unsafe { (this::<T, I>(p), &*iid) };
    trace(&alloc::format!("{}: QueryInterface {iid}", T::NAME));
    let slot = if *iid == IID_IUNKNOWN || *iid == IID_IINSPECTABLE || *iid == IID_IAGILEOBJECT {
        Some(0)
    } else {
        o.val.slot(iid)
    };
    match slot {
        Some(s) => {
            // SAFETY: checked non-null above.
            unsafe { *out = o.iface(s) };
            S_OK
        }
        None => {
            // SAFETY: as above.
            unsafe { *out = core::ptr::null_mut() };
            log(&alloc::format!("{}: no interface {iid}", T::NAME));
            E_NOINTERFACE
        }
    }
}

pub unsafe extern "system" fn add_ref<T: Class, const I: usize>(p: Ptr) -> u32 {
    // SAFETY: COM calls this on our interface pointer.
    unsafe { this::<T, I>(p) }
        .refs
        .fetch_add(1, Ordering::Relaxed)
        + 1
}

pub unsafe extern "system" fn release<T: Class, const I: usize>(p: Ptr) -> u32 {
    // SAFETY: COM calls this on our interface pointer.
    let o = unsafe { this::<T, I>(p) };
    let n = o.refs.fetch_sub(1, Ordering::Release) - 1;
    if n == 0 {
        core::sync::atomic::fence(Ordering::Acquire);
        // SAFETY: the last reference: the object came from Box::into_raw.
        drop(unsafe { Box::from_raw((o as *const Obj<T>).cast_mut()) });
    }
    n
}

pub unsafe extern "system" fn get_iids(_: Ptr, n: *mut u32, iids: *mut *mut Guid) -> Hresult {
    // SAFETY: out pointers from the caller.
    unsafe {
        put(n, 0);
        put(iids, core::ptr::null_mut())
    }
}

pub unsafe extern "system" fn class_name<T: Class>(_: Ptr, out: *mut Hstring) -> Hresult {
    let name: Vec<u16> = T::NAME.encode_utf16().collect();
    // SAFETY: out pointer from the caller.
    unsafe { put(out, hstring(&name)) }
}

pub unsafe extern "system" fn trust_level(_: Ptr, out: *mut i32) -> Hresult {
    // SAFETY: out pointer from the caller (BaseTrust).
    unsafe { put(out, 0) }
}

/// A vtable for slot `$i` of class `$t`: IInspectable, then `$m`.
macro_rules! vtable {
    ($t:ty, $i:literal; $($m:expr),* $(,)?) => {
        $crate::com::VTable([
            $crate::com::qi::<$t, $i> as *const core::ffi::c_void,
            $crate::com::add_ref::<$t, $i> as *const core::ffi::c_void,
            $crate::com::release::<$t, $i> as *const core::ffi::c_void,
            $crate::com::get_iids as *const core::ffi::c_void,
            $crate::com::class_name::<$t> as *const core::ffi::c_void,
            $crate::com::trust_level as *const core::ffi::c_void,
            $($m as *const core::ffi::c_void),*
        ])
    };
}
pub(crate) use vtable;

/// A method that is not provided: logged once per call.
macro_rules! notimpl {
    ($name:literal) => {{
        unsafe extern "system" fn f(_: $crate::com::Ptr) -> $crate::com::Hresult {
            $crate::sys::log(concat!("not implemented: ", $name));
            $crate::com::E_NOTIMPL
        }
        f
    }};
}
pub(crate) use notimpl;

/// A reference to a COM object (ours or a caller's): AddRef on clone,
/// Release on drop.
pub struct Unk(Ptr);

// SAFETY: the objects passed around here are agile (WE's delegates and
// buffers, our own objects).
unsafe impl Send for Unk {}
// SAFETY: as above.
unsafe impl Sync for Unk {}

impl Unk {
    /// Takes over a reference the caller owns.
    pub fn own(p: Ptr) -> Option<Self> {
        (!p.is_null()).then_some(Self(p))
    }

    /// A new reference to `p`.
    pub fn borrow(p: Ptr) -> Option<Self> {
        if p.is_null() {
            return None;
        }
        // SAFETY: a live COM object.
        unsafe { vcall::<unsafe extern "system" fn(Ptr) -> u32>(p, 1)(p) };
        Some(Self(p))
    }

    pub fn ptr(&self) -> Ptr {
        self.0
    }

    /// A new reference for an out parameter.
    pub fn out(&self) -> Ptr {
        core::mem::forget(self.clone());
        self.0
    }

    /// Its interface `iid` (a new reference), if it has it.
    pub fn cast(&self, iid: &Guid) -> Option<Self> {
        let mut out = core::ptr::null_mut();
        // SAFETY: QueryInterface, slot 0 of every COM vtable.
        let hr = unsafe {
            vcall::<unsafe extern "system" fn(Ptr, *const Guid, *mut Ptr) -> Hresult>(self.0, 0)(
                self.0, iid, &mut out,
            )
        };
        if hr == S_OK { Self::own(out) } else { None }
    }
}

impl Clone for Unk {
    fn clone(&self) -> Self {
        Self::borrow(self.0).expect("non-null")
    }
}

impl Drop for Unk {
    fn drop(&mut self) {
        // SAFETY: Release, slot 2 of every COM vtable.
        unsafe { vcall::<unsafe extern "system" fn(Ptr) -> u32>(self.0, 2)(self.0) };
    }
}

/// Method `slot` of the COM object `p`.
///
/// # Safety
/// `p` is a live COM object whose vtable has a method of type `F` there.
pub unsafe fn vcall<F: Copy>(p: Ptr, slot: usize) -> F {
    // SAFETY: per the caller.
    unsafe {
        let vt = *p.cast::<*const *const c_void>();
        core::mem::transmute_copy(&*vt.add(slot))
    }
}

/// An event's handlers (`TypedEventHandler<S, A>`: Invoke is slot 3).
pub struct Event {
    handlers: Lock<(i64, Vec<(i64, Unk)>)>,
}

impl Event {
    pub const fn new() -> Self {
        Self {
            handlers: Lock::new((0, Vec::new())),
        }
    }

    /// `add_X`.
    ///
    /// # Safety
    /// `handler` is a delegate (or null); `token` is valid for a write.
    pub unsafe fn add(&self, handler: Ptr, token: *mut i64) -> Hresult {
        let Some(h) = Unk::borrow(handler) else {
            return E_INVALIDARG;
        };
        let t = self.handlers.with(|(next, list)| {
            *next += 1;
            list.push((*next, h));
            *next
        });
        // SAFETY: per the caller.
        unsafe { put(token, t) }
    }

    /// `remove_X`.
    pub fn remove(&self, token: i64) -> Hresult {
        let gone = self.handlers.with(|(_, list)| {
            let i = list.iter().position(|(t, _)| *t == token);
            i.map(|i| list.remove(i))
        });
        drop(gone);
        S_OK
    }

    /// Calls every handler with `sender` and `args` (outside the lock).
    pub fn fire(&self, sender: Ptr, args: Ptr) {
        let list: Vec<Unk> = self
            .handlers
            .with(|(_, list)| list.iter().map(|(_, h)| h.clone()).collect());
        for h in list {
            // SAFETY: Invoke(sender, args) of a TypedEventHandler.
            unsafe {
                vcall::<unsafe extern "system" fn(Ptr, Ptr, Ptr) -> Hresult>(h.ptr(), 3)(
                    h.ptr(),
                    sender,
                    args,
                )
            };
        }
    }
}

/// What a completed operation gives.
pub enum Res {
    Obj(Option<Unk>),
    Bool(bool),
    U32(u32),
}

/// An `IAsyncOperation<T>` (or `IAsyncOperationWithProgress<T, P>`) that
/// is complete from the start: WE gets its result right away, through
/// `GetResults` or the `Completed` handler (called at once).
pub struct Async {
    iid: Guid,
    progress: bool,
    res: Res,
    completed: Lock<Option<Unk>>,
}

/// AsyncStatus::Completed.
const COMPLETED: i32 = 1;

impl Class for Async {
    const NAME: &'static str = "Windows.Foundation.IAsyncOperation`1";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        let op = if self.progress {
            ASYNC_PROGRESS_VT.ptr()
        } else {
            ASYNC_VT.ptr()
        };
        [op, ASYNC_INFO_VT.ptr(), op, op, op, op]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        if *iid == self.iid {
            Some(0)
        } else if *iid == IID_IASYNCINFO {
            Some(1)
        } else {
            None
        }
    }
}

/// A completed operation of interface `iid` giving `res`.
pub fn completed(iid: Guid, res: Res) -> Ptr {
    create(
        Async {
            iid,
            progress: false,
            res,
            completed: Lock::new(None),
        },
        0,
    )
}

/// A completed operation with progress of interface `iid` giving `res`.
pub fn completed_with_progress(iid: Guid, res: Res) -> Ptr {
    create(
        Async {
            iid,
            progress: true,
            res,
            completed: Lock::new(None),
        },
        0,
    )
}

unsafe extern "system" fn async_put_completed(p: Ptr, handler: Ptr) -> Hresult {
    trace("async_put_completed");
    // SAFETY: our interface pointer (slot 0).
    let o = unsafe { this::<Async, 0>(p) };
    let Some(h) = Unk::borrow(handler) else {
        return S_OK;
    };
    o.val.completed.with(|c| *c = Some(h.clone()));
    // AsyncOperationCompletedHandler::Invoke(asyncInfo, status), slot 3
    // SAFETY: a delegate passed by the caller.
    unsafe {
        vcall::<unsafe extern "system" fn(Ptr, Ptr, i32) -> Hresult>(h.ptr(), 3)(
            h.ptr(),
            p,
            COMPLETED,
        )
    };
    S_OK
}

unsafe extern "system" fn async_get_completed(p: Ptr, out: *mut Ptr) -> Hresult {
    trace("async_get_completed");
    // SAFETY: our interface pointer (slot 0).
    let o = unsafe { this::<Async, 0>(p) };
    let h = o.val.completed.with(|c| c.as_ref().map(Unk::out));
    // SAFETY: out pointer from the caller.
    unsafe { put(out, h.unwrap_or(core::ptr::null_mut())) }
}

unsafe extern "system" fn async_get_results(p: Ptr, out: *mut c_void) -> Hresult {
    trace("async_get_results");
    // SAFETY: our interface pointer (slot 0).
    let o = unsafe { this::<Async, 0>(p) };
    // SAFETY: out is a T* of the operation's result type.
    unsafe {
        match &o.val.res {
            Res::Obj(v) => put(
                out.cast::<Ptr>(),
                v.as_ref().map_or(core::ptr::null_mut(), Unk::out),
            ),
            Res::Bool(b) => put(out.cast::<bool>(), *b),
            Res::U32(n) => put(out.cast::<u32>(), *n),
        }
    }
}

unsafe extern "system" fn async_put_progress(_: Ptr, _: Ptr) -> Hresult {
    S_OK
}

unsafe extern "system" fn async_get_progress(_: Ptr, out: *mut Ptr) -> Hresult {
    // SAFETY: out pointer from the caller.
    unsafe { put(out, core::ptr::null_mut()) }
}

unsafe extern "system" fn info_id(_: Ptr, out: *mut u32) -> Hresult {
    // SAFETY: out pointer from the caller.
    unsafe { put(out, 1) }
}

unsafe extern "system" fn info_status(_: Ptr, out: *mut i32) -> Hresult {
    trace("info_status");
    // SAFETY: out pointer from the caller.
    unsafe { put(out, COMPLETED) }
}

unsafe extern "system" fn info_error(_: Ptr, out: *mut Hresult) -> Hresult {
    // SAFETY: out pointer from the caller.
    unsafe { put(out, S_OK) }
}

unsafe extern "system" fn info_nothing(_: Ptr) -> Hresult {
    S_OK
}

static ASYNC_VT: VTable<9> =
    vtable!(Async, 0; async_put_completed, async_get_completed, async_get_results);
static ASYNC_PROGRESS_VT: VTable<11> = vtable!(Async, 0;
    async_put_progress, async_get_progress, async_put_completed, async_get_completed, async_get_results);
static ASYNC_INFO_VT: VTable<11> =
    vtable!(Async, 1; info_id, info_status, info_error, info_nothing, info_nothing);
