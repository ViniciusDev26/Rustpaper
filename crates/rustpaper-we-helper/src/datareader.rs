//! `Windows.Storage.Streams.DataReader` over our album art stream.
//!
//! WE's helper reads the thumbnail with `DataReader(stream)` (C++/WinRT),
//! and Wine's `DataReaderFactory::CreateDataReader(IInputStream)` is a stub
//! (wintypes.dll: "iface %p, input_stream %p, data_reader %p stub!",
//! GE-Proton 11-13): it failed with E_NOTIMPL and the album art never
//! reached WE (the trace stopped at the stream's IInputStream, 2026-11-27).
//! So the engine registers this DLL for that class too. For our own stream
//! (recognized by a private interface) it hands out a reader over its bytes;
//! anything else, and `FromBuffer`, goes to Wine's own factory as before.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;

use crate::com::{
    Class, E_BOUNDS, E_INVALIDARG, E_NOTIMPL, Guid, Hresult, Hstring, IID_IBUFFERBYTEACCESS, Ptr,
    Res, S_OK, SLOTS, Unk, VTable, completed, create, hstring, notimpl, put, this, vcall, vtable,
};
use crate::sys::{Lock, log, trace};

pub const CLASS: &str = "Windows.Storage.Streams.DataReader";

const IID_ACTIVATION_FACTORY: Guid = Guid::new(0x00000035_0000_0000_c000_000000000046);
const IID_FACTORY: Guid = Guid::new(0xd7527847_57da_4e15_914c_06806699a098);
const IID_STATICS: Guid = Guid::new(0x11fcbfc8_f93a_471b_b121_f379e349313c);
const IID_READER: Guid = Guid::new(0xe2b50029_b4c1_4314_a4b8_fb813a2f275e);
const IID_CLOSABLE: Guid = Guid::new(0x30d5a829_7fa4_4026_83bb_d75bae4ea99e);
const IID_BUFFER: Guid = Guid::new(0x905a0fe0_bc53_11df_8c49_001e4fc686da);
/// IAsyncOperation<UInt32> (DataReaderLoadOperation).
const IID_ASYNC_U32: Guid = Guid::new(0xef60385f_be78_584b_aaef_7829ada2b0de);
/// Private: our album art stream answers it (gsmtc.rs), with its bytes.
pub const IID_OUR_BYTES: Guid = Guid::new(0x46b22177_7c7a_4d75_9bff_c1b123b74353);

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

type GetFactory = unsafe extern "system" fn(Hstring, *mut Ptr) -> Hresult;

/// Wine's own DataReader factory (wintypes.dll), for what we don't handle.
fn wine_factory() -> Option<Unk> {
    static WINE: Lock<Option<Option<Unk>>> = Lock::new(None);
    WINE.with(|w| {
        w.get_or_insert_with(|| {
            let dll: Vec<u16> = "wintypes.dll\0".encode_utf16().collect();
            // SAFETY: plain loader calls; the export has the documented
            // DllGetActivationFactory signature.
            unsafe {
                let m = LoadLibraryW(dll.as_ptr());
                let f = GetProcAddress(m, c"DllGetActivationFactory".as_ptr().cast());
                if m.is_null() || f.is_null() {
                    log("DataReader: wintypes.dll's factory not found");
                    return None;
                }
                let f: GetFactory = core::mem::transmute(f);
                let name: Vec<u16> = CLASS.encode_utf16().collect();
                let h = hstring(&name);
                let mut out = core::ptr::null_mut();
                f(h, &mut out);
                Unk::own(out)
            }
        })
        .clone()
    })
}

/// The activation factory: IActivationFactory (0), IDataReaderFactory (1),
/// IDataReaderStatics (2).
pub struct Factory;

impl Class for Factory {
    const NAME: &'static str = CLASS;
    fn vtables(&self) -> [*const c_void; SLOTS] {
        let a = ACTIVATION_VT.ptr();
        [a, FACTORY_VT.ptr(), STATICS_VT.ptr(), a, a, a]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        [IID_ACTIVATION_FACTORY, IID_FACTORY, IID_STATICS]
            .iter()
            .position(|i| i == iid)
    }
}

unsafe extern "system" fn create_data_reader(_: Ptr, stream: Ptr, out: *mut Ptr) -> Hresult {
    trace("DataReader::CreateDataReader");
    let ours = Unk::borrow(stream).and_then(|s| s.cast(&IID_OUR_BYTES));
    if let Some(o) = ours {
        let (data, pos) = crate::gsmtc::stream_bytes(o.ptr());
        let r = Reader {
            data,
            state: Lock::new(ReaderState {
                consumed: pos,
                loaded: pos,
                byte_order: 1, // BigEndian, DataReader's default
                encoding: 0,   // Utf8
                options: 0,
            }),
        };
        // SAFETY: out pointer from the caller.
        return unsafe { put(out, create(r, 0)) };
    }
    // not ours: Wine's (a stub there, as without this DLL)
    let Some(f) = wine_factory().and_then(|f| f.cast(&IID_FACTORY)) else {
        return E_NOTIMPL;
    };
    // SAFETY: IDataReaderFactory::CreateDataReader (slot 6) on Wine's factory.
    unsafe {
        vcall::<unsafe extern "system" fn(Ptr, Ptr, *mut Ptr) -> Hresult>(f.ptr(), 6)(
            f.ptr(),
            stream,
            out,
        )
    }
}

unsafe extern "system" fn from_buffer(_: Ptr, buffer: Ptr, out: *mut Ptr) -> Hresult {
    trace("DataReader::FromBuffer");
    let Some(f) = wine_factory().and_then(|f| f.cast(&IID_STATICS)) else {
        return E_NOTIMPL;
    };
    // SAFETY: IDataReaderStatics::FromBuffer (slot 6) on Wine's factory.
    unsafe {
        vcall::<unsafe extern "system" fn(Ptr, Ptr, *mut Ptr) -> Hresult>(f.ptr(), 6)(
            f.ptr(),
            buffer,
            out,
        )
    }
}

static ACTIVATION_VT: VTable<7> = vtable!(Factory, 0; notimpl!("DataReader::ActivateInstance"));
static FACTORY_VT: VTable<7> = vtable!(Factory, 1; create_data_reader);
static STATICS_VT: VTable<7> = vtable!(Factory, 2; from_buffer);

struct ReaderState {
    /// Read up to here.
    consumed: usize,
    /// Loaded (LoadAsync) up to here.
    loaded: usize,
    /// ByteOrder: 0 little, 1 big endian.
    byte_order: i32,
    /// UnicodeEncoding: 0 UTF-8, 1 UTF-16LE, 2 UTF-16BE.
    encoding: i32,
    options: u32,
}

/// A DataReader over bytes in memory: IDataReader (0), IClosable (1).
struct Reader {
    data: Arc<Vec<u8>>,
    state: Lock<ReaderState>,
}

impl Class for Reader {
    const NAME: &'static str = CLASS;
    fn vtables(&self) -> [*const c_void; SLOTS] {
        let r = READER_VT.ptr();
        [r, CLOSABLE_VT.ptr(), r, r, r, r]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        [IID_READER, IID_CLOSABLE].iter().position(|i| i == iid)
    }
}

/// The reader behind slot 0.
unsafe fn reader<'a>(p: Ptr) -> &'a Reader {
    // SAFETY: per the caller.
    unsafe { &this::<Reader, 0>(p).val }
}

impl Reader {
    /// Takes the next `n` loaded bytes.
    fn take(&self, n: usize) -> Option<Vec<u8>> {
        self.state.with(|s| {
            if s.loaded - s.consumed < n {
                return None;
            }
            let v = self.data[s.consumed..s.consumed + n].to_vec();
            s.consumed += n;
            Some(v)
        })
    }

    /// The next `N` bytes as a number in the reader's byte order, little
    /// endian out.
    fn number<const N: usize>(&self) -> Option<[u8; N]> {
        let mut b: [u8; N] = self.take(N)?.try_into().ok()?;
        if self.state.with(|s| s.byte_order) == 1 {
            b.reverse();
        }
        Some(b)
    }
}

unsafe extern "system" fn unconsumed(p: Ptr, out: *mut u32) -> Hresult {
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe {
        let n = reader(p).state.with(|s| s.loaded - s.consumed);
        put(out, n as u32)
    }
}

unsafe extern "system" fn get_encoding(p: Ptr, out: *mut i32) -> Hresult {
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, reader(p).state.with(|s| s.encoding)) }
}

unsafe extern "system" fn set_encoding(p: Ptr, v: i32) -> Hresult {
    // SAFETY: our interface pointer.
    unsafe { reader(p) }.state.with(|s| s.encoding = v);
    S_OK
}

unsafe extern "system" fn get_byte_order(p: Ptr, out: *mut i32) -> Hresult {
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, reader(p).state.with(|s| s.byte_order)) }
}

unsafe extern "system" fn set_byte_order(p: Ptr, v: i32) -> Hresult {
    // SAFETY: our interface pointer.
    unsafe { reader(p) }.state.with(|s| s.byte_order = v);
    S_OK
}

unsafe extern "system" fn get_options(p: Ptr, out: *mut u32) -> Hresult {
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, reader(p).state.with(|s| s.options)) }
}

unsafe extern "system" fn set_options(p: Ptr, v: u32) -> Hresult {
    // SAFETY: our interface pointer.
    unsafe { reader(p) }.state.with(|s| s.options = v);
    S_OK
}

unsafe extern "system" fn read_bytes(p: Ptr, n: u32, out: *mut u8) -> Hresult {
    trace("DataReader::ReadBytes");
    // SAFETY: our interface pointer.
    let Some(v) = unsafe { reader(p) }.take(n as usize) else {
        return E_BOUNDS;
    };
    if !v.is_empty() {
        // SAFETY: the caller's array holds n bytes.
        unsafe { core::ptr::copy_nonoverlapping(v.as_ptr(), out, v.len()) };
    }
    S_OK
}

macro_rules! read_number {
    ($name:ident, $t:ty) => {
        unsafe extern "system" fn $name(p: Ptr, out: *mut $t) -> Hresult {
            // SAFETY: our interface pointer.
            match unsafe { reader(p) }.number::<{ core::mem::size_of::<$t>() }>() {
                // SAFETY: out pointer from the caller.
                Some(b) => unsafe { put(out, <$t>::from_le_bytes(b)) },
                None => E_BOUNDS,
            }
        }
    };
}

read_number!(read_byte, u8);
read_number!(read_i16, i16);
read_number!(read_i32, i32);
read_number!(read_i64, i64);
read_number!(read_u16, u16);
read_number!(read_u32, u32);
read_number!(read_u64, u64);
read_number!(read_f32, f32);
read_number!(read_f64, f64);

unsafe extern "system" fn read_bool(p: Ptr, out: *mut bool) -> Hresult {
    // SAFETY: our interface pointer.
    match unsafe { reader(p) }.take(1) {
        // SAFETY: out pointer from the caller.
        Some(b) => unsafe { put(out, b[0] != 0) },
        None => E_BOUNDS,
    }
}

unsafe extern "system" fn read_guid(p: Ptr, out: *mut [u8; 16]) -> Hresult {
    // SAFETY: our interface pointer.
    match unsafe { reader(p) }.take(16) {
        // SAFETY: out pointer from the caller.
        Some(b) => unsafe { put(out, b.try_into().unwrap_or([0; 16])) },
        None => E_BOUNDS,
    }
}

/// `ReadString(codeUnitCount)`: that many UTF-8 bytes or UTF-16 units.
unsafe extern "system" fn read_string(p: Ptr, units: u32, out: *mut Hstring) -> Hresult {
    // SAFETY: our interface pointer.
    let r = unsafe { reader(p) };
    let encoding = r.state.with(|s| s.encoding);
    let n = units as usize;
    let wide: Vec<u16> = if encoding == 0 {
        let Some(b) = r.take(n) else {
            return E_BOUNDS;
        };
        alloc::string::String::from_utf8_lossy(&b)
            .encode_utf16()
            .collect()
    } else {
        let Some(b) = r.take(n * 2) else {
            return E_BOUNDS;
        };
        b.as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if encoding == 2 {
                    u16::from_be_bytes(c)
                } else {
                    u16::from_le_bytes(c)
                }
            })
            .collect()
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, hstring(&wide)) }
}

/// `LoadAsync(count)`: makes up to `count` more bytes readable; the bytes
/// are in memory already, so it completes at once.
unsafe extern "system" fn load_async(p: Ptr, count: u32, out: *mut Ptr) -> Hresult {
    trace("DataReader::LoadAsync");
    // SAFETY: our interface pointer.
    let r = unsafe { reader(p) };
    let n = r.state.with(|s| {
        let end = (s.loaded + count as usize).min(r.data.len());
        let n = end - s.loaded;
        s.loaded = end;
        n
    });
    // SAFETY: out pointer from the caller.
    unsafe { put(out, completed(IID_ASYNC_U32, Res::U32(n as u32))) }
}

/// `ReadBuffer(length)`: the next bytes in a new IBuffer.
unsafe extern "system" fn read_buffer(p: Ptr, n: u32, out: *mut Ptr) -> Hresult {
    trace("DataReader::ReadBuffer");
    // SAFETY: our interface pointer.
    let Some(v) = unsafe { reader(p) }.take(n as usize) else {
        return E_BOUNDS;
    };
    let len = v.len() as u32;
    let b = Buffer {
        bytes: Lock::new((v, len)),
    };
    // SAFETY: out pointer from the caller.
    unsafe { put(out, create(b, 0)) }
}

/// A `Windows.Storage.Streams.Buffer` of bytes: IBuffer (0),
/// IBufferByteAccess (1, an IUnknown interface).
struct Buffer {
    /// The bytes (capacity) and the length in use.
    bytes: Lock<(Vec<u8>, u32)>,
}

impl Class for Buffer {
    const NAME: &'static str = "Windows.Storage.Streams.Buffer";
    fn vtables(&self) -> [*const c_void; SLOTS] {
        let b = BUFFER_VT.ptr();
        [b, BYTE_ACCESS_VT.ptr(), b, b, b, b]
    }
    fn slot(&self, iid: &Guid) -> Option<usize> {
        [IID_BUFFER, IID_IBUFFERBYTEACCESS]
            .iter()
            .position(|i| i == iid)
    }
}

unsafe extern "system" fn buffer_capacity(p: Ptr, out: *mut u32) -> Hresult {
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe {
        put(
            out,
            this::<Buffer, 0>(p).val.bytes.with(|(v, _)| v.len() as u32),
        )
    }
}

unsafe extern "system" fn buffer_length(p: Ptr, out: *mut u32) -> Hresult {
    // SAFETY: our interface pointer; out pointer from the caller.
    unsafe { put(out, this::<Buffer, 0>(p).val.bytes.with(|(_, n)| *n)) }
}

unsafe extern "system" fn buffer_set_length(p: Ptr, n: u32) -> Hresult {
    // SAFETY: our interface pointer.
    let ok = unsafe { this::<Buffer, 0>(p) }.val.bytes.with(|(v, len)| {
        let ok = n as usize <= v.len();
        if ok {
            *len = n;
        }
        ok
    });
    if ok { S_OK } else { E_INVALIDARG }
}

unsafe extern "system" fn buffer_bytes(p: Ptr, out: *mut *mut u8) -> Hresult {
    // SAFETY: our interface pointer (slot 1). The bytes never move: the
    // vector is never resized.
    let ptr = unsafe { this::<Buffer, 1>(p) }
        .val
        .bytes
        .with(|(v, _)| v.as_mut_ptr());
    // SAFETY: out pointer from the caller.
    unsafe { put(out, ptr) }
}

static BUFFER_VT: VTable<9> = vtable!(Buffer, 0; buffer_capacity, buffer_length, buffer_set_length);
static BYTE_ACCESS_VT: VTable<4> = VTable([
    crate::com::qi::<Buffer, 1> as *const c_void,
    crate::com::add_ref::<Buffer, 1> as *const c_void,
    crate::com::release::<Buffer, 1> as *const c_void,
    buffer_bytes as *const c_void,
]);

unsafe extern "system" fn closed(_: Ptr) -> Hresult {
    S_OK
}

static READER_VT: VTable<32> = vtable!(Reader, 0;
    unconsumed, get_encoding, set_encoding, get_byte_order, set_byte_order,
    get_options, set_options, read_byte, read_bytes,
    read_buffer,
    read_bool, read_guid, read_i16, read_i32, read_i64, read_u16, read_u32, read_u64,
    read_f32, read_f64, read_string,
    read_i64, // ReadDateTime: an i64 of 100 ns ticks
    read_i64, // ReadTimeSpan: likewise
    load_async,
    notimpl!("DataReader::DetachBuffer"),
    notimpl!("DataReader::DetachStream"));
static CLOSABLE_VT: VTable<7> = vtable!(Reader, 1; closed);
