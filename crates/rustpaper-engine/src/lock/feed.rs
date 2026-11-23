//! The frame feed: the engine hands Wallpaper Engine's frames to the lock
//! screen, so the lock shows the same live wallpaper without rendering
//! anything itself.
//!
//! With the private stage the engine holds WE's GPU buffers (dmabufs). It
//! sends them over a Unix `SOCK_SEQPACKET` socket (one message per frame,
//! boundaries kept, file descriptors attached with `SCM_RIGHTS`); the lock
//! imports them on the user's compositor like the mirror does. Zero copy:
//! only descriptors and a few numbers cross the socket.
//!
//! - The fds of a buffer are sent the first time the client sees its key on
//!   a monitor; later frames of the same buffer carry only the key (WE
//!   cycles through a few buffers). Per monitor: with shared rendering one
//!   WE buffer shows on several monitors (main.rs).
//! - The client answers `release` (key and monitor) when its compositor
//!   released the buffer; only then may WE reuse it (the engine also waits
//!   for the mirror).
//! - `gone`: WE destroyed the buffer; the key may come back for a new one.
//! - The client sends `pointer` (monitor and the input, as fractions of the
//!   monitor) while the user's pointer is over the wallpaper, so it follows
//!   the cursor on the lock screen too (main.rs hands it to the private
//!   stage). Only the pointer: the keyboard types the password and never
//!   leaves the lock.
//!
//! Messages are JSON (`serde_json`); modifiers are split into two u32
//! because JSON numbers lose precision above 2^53.
//!
//! The socket is per Wayland display (`feed-<WAYLAND_DISPLAY>.sock` in
//! `$XDG_RUNTIME_DIR/rustpaper`), so a test engine on a nested compositor
//! never feeds the real session's lock, and the server only accepts peers of
//! its own uid.

use std::collections::HashSet;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::stage::{Frame, FramePixels, PointerInput};

/// Largest message (JSON of one frame is ~200 bytes).
const MAX_MSG: usize = 4096;
/// Most fds one frame carries (dmabufs have at most 4 planes).
const MAX_FDS: usize = 4;

/// `$XDG_RUNTIME_DIR/rustpaper/feed-<WAYLAND_DISPLAY>.sock`.
pub fn socket_path() -> Option<PathBuf> {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    // a display may be an absolute path: keep only a file-name-safe form
    let name: String = display
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    Some(runtime.join("rustpaper").join(format!("feed-{name}.sock")))
}

fn sockaddr(path: &std::path::Path) -> Option<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: an all-zero sockaddr_un is valid.
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= addr.sun_path.len() {
        return None;
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (d, s) in addr.sun_path.iter_mut().zip(bytes) {
        *d = *s as libc::c_char;
    }
    Some((
        addr,
        std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
    ))
}

fn seqpacket() -> Option<OwnedFd> {
    // SAFETY: plain socket(2); the fd is owned right away.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        )
    };
    // SAFETY: a fresh fd we own.
    (fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Sends one message with `fds` attached. False when it could not be sent
/// whole (peer gone, or its queue full: the frame is simply skipped).
fn send(sock: RawFd, msg: &[u8], fds: &[RawFd]) -> bool {
    let mut iov = libc::iovec {
        iov_base: msg.as_ptr() as *mut libc::c_void,
        iov_len: msg.len(),
    };
    // SAFETY: CMSG_SPACE is a pure computation.
    let space = unsafe { libc::CMSG_SPACE((std::mem::size_of::<RawFd>() * MAX_FDS) as u32) };
    let mut cbuf = vec![0u8; space as usize];
    // SAFETY: an all-zero msghdr is valid; the pointers below outlive the call.
    let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
    hdr.msg_iov = &mut iov;
    hdr.msg_iovlen = 1;
    if !fds.is_empty() {
        let fds = &fds[..fds.len().min(MAX_FDS)];
        let len = std::mem::size_of_val(fds);
        hdr.msg_control = cbuf.as_mut_ptr().cast();
        // SAFETY: as above.
        hdr.msg_controllen = unsafe { libc::CMSG_SPACE(len as u32) } as _;
        // SAFETY: cbuf holds a cmsg header plus `len` bytes of data.
        unsafe {
            let c = libc::CMSG_FIRSTHDR(&hdr);
            (*c).cmsg_level = libc::SOL_SOCKET;
            (*c).cmsg_type = libc::SCM_RIGHTS;
            (*c).cmsg_len = libc::CMSG_LEN(len as u32) as _;
            std::ptr::copy_nonoverlapping(fds.as_ptr(), libc::CMSG_DATA(c).cast(), fds.len());
        }
    }
    // SAFETY: a valid msghdr; MSG_NOSIGNAL: a gone peer is an error, not SIGPIPE.
    let n = unsafe { libc::sendmsg(sock, &hdr, libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT) };
    n == msg.len() as isize
}

enum Recv {
    Msg(Vec<u8>, Vec<OwnedFd>),
    /// Nothing queued.
    Empty,
    /// The peer closed.
    Closed,
}

fn recv(sock: RawFd) -> Recv {
    let mut buf = vec![0u8; MAX_MSG];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    // SAFETY: pure computation.
    let space = unsafe { libc::CMSG_SPACE((std::mem::size_of::<RawFd>() * MAX_FDS) as u32) };
    let mut cbuf = vec![0u8; space as usize];
    // SAFETY: as in `send`.
    let mut hdr: libc::msghdr = unsafe { std::mem::zeroed() };
    hdr.msg_iov = &mut iov;
    hdr.msg_iovlen = 1;
    hdr.msg_control = cbuf.as_mut_ptr().cast();
    hdr.msg_controllen = cbuf.len() as _;
    // SAFETY: valid msghdr; received fds are close-on-exec.
    let n = unsafe { libc::recvmsg(sock, &mut hdr, libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        let e = std::io::Error::last_os_error();
        return match e.kind() {
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted => Recv::Empty,
            _ => Recv::Closed,
        };
    }
    if n == 0 {
        return Recv::Closed;
    }
    let mut fds = Vec::new();
    // SAFETY: walking the control messages the kernel just wrote.
    unsafe {
        let mut c = libc::CMSG_FIRSTHDR(&hdr);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_RIGHTS {
                let data = libc::CMSG_DATA(c) as *const RawFd;
                let count = ((*c).cmsg_len as usize - libc::CMSG_LEN(0) as usize)
                    / std::mem::size_of::<RawFd>();
                for i in 0..count {
                    fds.push(OwnedFd::from_raw_fd(std::ptr::read_unaligned(data.add(i))));
                }
            }
            c = libc::CMSG_NXTHDR(&hdr, c);
        }
    }
    buf.truncate(n as usize);
    Recv::Msg(buf, fds)
}

// ---- engine side -----------------------------------------------------------

/// Who still uses one of WE's buffers, as bits: one per monitor's mirror,
/// one per monitor on the lock screen. It goes back to WE when nobody does.
pub mod holders {
    /// Bit of mirror `i`.
    pub fn mirror(i: usize) -> u64 {
        1 << (i % 32)
    }
    /// Bit of mirror `i`'s monitor on the lock screen.
    pub fn lock(i: usize) -> u64 {
        1 << (32 + i % 32)
    }
}

struct Client {
    fd: OwnedFd,
    /// (monitor, key) whose fds this client has.
    known: HashSet<(String, u64)>,
    /// (monitor, key) it holds (sent, not yet released).
    held: HashSet<(String, u64)>,
}

/// The engine's end: one lock client at a time (a new one replaces it).
pub struct FeedServer {
    listener: OwnedFd,
    path: PathBuf,
    /// The socket file's inode (to remove only our own on exit).
    inode: u64,
    client: Option<Client>,
    /// Pointer input from the lock since the last `take_pointer`.
    pointer: Vec<(String, PointerInput)>,
}

impl FeedServer {
    /// Listens on [`socket_path`]; None (logged) when it cannot.
    pub fn bind() -> Option<Self> {
        Self::bind_at(socket_path()?)
    }

    fn bind_at(path: PathBuf) -> Option<Self> {
        let dir = path.parent()?;
        let _ = std::fs::create_dir_all(dir);
        // a previous engine's socket (an engine handing over removes it last)
        let _ = std::fs::remove_file(&path);
        let listener = seqpacket()?;
        let (addr, len) = sockaddr(&path)?;
        // SAFETY: a valid sockaddr_un of `len` bytes.
        let ok = unsafe {
            libc::bind(
                listener.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                len,
            ) == 0
                && libc::listen(listener.as_raw_fd(), 4) == 0
        };
        if !ok {
            eprintln!(
                "lock feed: cannot listen on {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            );
            return None;
        }
        use std::os::unix::fs::MetadataExt;
        let inode = std::fs::metadata(&path).map(|m| m.ino()).unwrap_or(0);
        Some(FeedServer {
            listener,
            path,
            inode,
            client: None,
            pointer: Vec::new(),
        })
    }

    /// The fds to wait on (listener, client).
    pub fn poll_fds(&self) -> [RawFd; 2] {
        [
            self.listener.as_raw_fd(),
            self.client.as_ref().map_or(-1, |c| c.fd.as_raw_fd()),
        ]
    }

    /// A lock screen is showing the frames.
    pub fn active(&self) -> bool {
        self.client.is_some()
    }

    /// Accepts a new client and reads releases. Returns the keys the lock no
    /// longer holds (released, or everything it held when it went away), and
    /// whether a client connected or left.
    pub fn poll(&mut self) -> (Vec<(String, u64)>, bool) {
        let mut freed = Vec::new();
        let mut changed = false;
        loop {
            // SAFETY: accept4 on our listener; the fd is owned right away.
            let fd = unsafe {
                libc::accept4(
                    self.listener.as_raw_fd(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                )
            };
            if fd < 0 {
                break;
            }
            // SAFETY: fresh fd from accept4.
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            if !same_user(fd.as_raw_fd()) {
                continue;
            }
            if let Some(old) = self.client.take() {
                freed.extend(old.held);
            }
            eprintln!("lock feed: the lock screen is showing the wallpaper");
            self.client = Some(Client {
                fd,
                known: HashSet::new(),
                held: HashSet::new(),
            });
            changed = true;
        }
        let mut closed = false;
        if let Some(c) = &mut self.client {
            loop {
                match recv(c.fd.as_raw_fd()) {
                    Recv::Msg(msg, _) => {
                        let Ok(v) = serde_json::from_slice::<Value>(&msg) else {
                            continue;
                        };
                        if v["t"] == "release"
                            && let Some(key) = v["key"].as_u64()
                            && let Some(output) = v["output"].as_str()
                        {
                            let k = (output.to_string(), key);
                            if c.held.remove(&k) {
                                freed.push(k);
                            }
                        } else if v["t"] == "pointer"
                            && let Some(output) = v["output"].as_str()
                            && let Some(input) = pointer_from_json(&v)
                        {
                            self.pointer.push((output.to_string(), input));
                        }
                    }
                    Recv::Empty => break,
                    Recv::Closed => {
                        closed = true;
                        break;
                    }
                }
            }
        }
        if closed && let Some(c) = self.client.take() {
            eprintln!("lock feed: the lock screen closed");
            freed.extend(c.held);
            changed = true;
        }
        (freed, changed)
    }

    /// Pointer input from the lock screen since the last call.
    pub fn take_pointer(&mut self) -> Vec<(String, PointerInput)> {
        std::mem::take(&mut self.pointer)
    }

    /// Sends a frame of the private stage. True when the lock now holds
    /// WE's buffer (dmabuf frames only: shm frames are not shared).
    pub fn send(&mut self, frame: &Frame) -> bool {
        let Some(c) = &mut self.client else {
            return false;
        };
        let FramePixels::Dmabuf {
            fourcc,
            modifier,
            planes,
        } = &frame.pixels
        else {
            return false;
        };
        let k = (frame.output.clone(), frame.key);
        let with_fds = !c.known.contains(&k);
        let msg = json!({
            "t": "frame",
            "output": frame.output,
            "key": frame.key,
            "w": frame.size.0,
            "h": frame.size.1,
            "fourcc": fourcc,
            "mod_hi": (modifier >> 32) as u32,
            "mod_lo": *modifier as u32,
            "planes": planes.iter().map(|p| [p.offset, p.stride]).collect::<Vec<_>>(),
            "fds": with_fds,
        });
        let fds: Vec<RawFd> = if with_fds {
            planes.iter().map(|p| p.fd.as_raw_fd()).collect()
        } else {
            Vec::new()
        };
        if !send(c.fd.as_raw_fd(), msg.to_string().as_bytes(), &fds) {
            return false;
        }
        if with_fds {
            c.known.insert(k.clone());
        }
        c.held.insert(k);
        true
    }

    /// WE destroyed buffer `key`.
    pub fn gone(&mut self, key: u64) {
        if let Some(c) = &mut self.client
            && c.known.iter().any(|(_, k)| *k == key)
        {
            c.known.retain(|(_, k)| *k != key);
            let msg = json!({"t": "gone", "key": key});
            send(c.fd.as_raw_fd(), msg.to_string().as_bytes(), &[]);
        }
    }
}

impl Drop for FeedServer {
    fn drop(&mut self) {
        // only our own socket (a newer engine may have replaced it)
        use std::os::unix::fs::MetadataExt;
        if std::fs::metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// The peer runs as our uid (`SO_PEERCRED`).
fn same_user(fd: RawFd) -> bool {
    // SAFETY: an all-zero ucred is valid.
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: getsockopt into a correctly sized ucred.
    let ok = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        ) == 0
    };
    // SAFETY: getuid cannot fail.
    ok && cred.uid == unsafe { libc::getuid() }
}

// ---- lock side -------------------------------------------------------------

/// A frame for the lock screen.
pub struct FeedFrame {
    pub output: String,
    pub key: u64,
    pub size: (u32, u32),
    pub fourcc: u32,
    pub modifier: u64,
    /// (offset, stride) per plane.
    pub planes: Vec<(u32, u32)>,
    /// The planes' fds, the first time this key is sent (else empty).
    pub fds: Vec<OwnedFd>,
}

pub enum FeedEvent {
    Frame(FeedFrame),
    Gone(u64),
}

/// The lock's end of the feed.
pub struct FeedClient {
    fd: OwnedFd,
}

impl FeedClient {
    /// Connects to the engine of this Wayland display; None when no engine
    /// (with the private stage) is running.
    pub fn connect() -> Option<Self> {
        Self::connect_at(&socket_path()?)
    }

    fn connect_at(path: &std::path::Path) -> Option<Self> {
        let fd = seqpacket()?;
        let (addr, len) = sockaddr(path)?;
        // SAFETY: a valid sockaddr_un of `len` bytes.
        let ok = unsafe {
            libc::connect(
                fd.as_raw_fd(),
                (&addr as *const libc::sockaddr_un).cast(),
                len,
            ) == 0
        };
        ok.then_some(FeedClient { fd })
    }

    pub fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Everything queued; Err when the engine went away.
    pub fn read(&self) -> Result<Vec<FeedEvent>, ()> {
        let mut out = Vec::new();
        loop {
            match recv(self.fd.as_raw_fd()) {
                Recv::Msg(msg, fds) => {
                    if let Some(e) = parse(&msg, fds) {
                        out.push(e);
                    }
                }
                Recv::Empty => return Ok(out),
                Recv::Closed => return Err(()),
            }
        }
    }

    /// The compositor released the buffer of `key`.
    pub fn release(&self, output: &str, key: u64) {
        let msg = json!({"t": "release", "key": key, "output": output});
        send(self.fd.as_raw_fd(), msg.to_string().as_bytes(), &[]);
    }

    /// The user's pointer over monitor `output`'s wallpaper.
    pub fn pointer(&self, output: &str, input: PointerInput) {
        let mut msg = pointer_json(input);
        msg["t"] = json!("pointer");
        msg["output"] = json!(output);
        send(self.fd.as_raw_fd(), msg.to_string().as_bytes(), &[]);
    }
}

fn pointer_json(input: PointerInput) -> Value {
    match input {
        PointerInput::Motion { x, y } => json!({"kind": "motion", "x": x, "y": y}),
        PointerInput::Button { button, pressed } => {
            json!({"kind": "button", "button": button, "pressed": pressed})
        }
        PointerInput::Axis { vertical, value } => {
            json!({"kind": "axis", "vertical": vertical, "value": value})
        }
        PointerInput::Leave => json!({"kind": "leave"}),
    }
}

fn pointer_from_json(v: &Value) -> Option<PointerInput> {
    Some(match v["kind"].as_str()? {
        "motion" => PointerInput::Motion {
            x: v["x"].as_f64()?,
            y: v["y"].as_f64()?,
        },
        "button" => PointerInput::Button {
            button: u32::try_from(v["button"].as_u64()?).ok()?,
            pressed: v["pressed"].as_bool()?,
        },
        "axis" => PointerInput::Axis {
            vertical: v["vertical"].as_bool()?,
            value: v["value"].as_f64()?,
        },
        "leave" => PointerInput::Leave,
        _ => return None,
    })
}

fn parse(msg: &[u8], fds: Vec<OwnedFd>) -> Option<FeedEvent> {
    let v: Value = serde_json::from_slice(msg).ok()?;
    let u32_of = |k: &str| v[k].as_u64().map(|n| n as u32);
    match v["t"].as_str()? {
        "frame" => {
            let planes = v["planes"]
                .as_array()?
                .iter()
                .filter_map(|p| Some((p[0].as_u64()? as u32, p[1].as_u64()? as u32)))
                .collect();
            Some(FeedEvent::Frame(FeedFrame {
                output: v["output"].as_str()?.to_string(),
                key: v["key"].as_u64()?,
                size: (u32_of("w")?, u32_of("h")?),
                fourcc: u32_of("fourcc")?,
                modifier: ((u32_of("mod_hi")? as u64) << 32) | u32_of("mod_lo")? as u64,
                planes,
                fds,
            }))
        }
        "gone" => Some(FeedEvent::Gone(v["key"].as_u64()?)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage::Plane;

    /// A frame and its fds survive the socket; later frames of the same
    /// buffer carry no fds; releases and pointer input come back; a closed
    /// lock frees all.
    #[test]
    fn frames_cross_the_socket() {
        let dir = std::env::temp_dir().join(format!("rp-feed-{}", std::process::id()));
        let path = dir.join("feed-test.sock");
        let mut server = FeedServer::bind_at(path.clone()).expect("bind");
        let client = FeedClient::connect_at(&path).expect("connect");
        let (_, changed) = server.poll();
        assert!(changed && server.active());

        let file = std::fs::File::open("/dev/null").unwrap();
        let frame = |key| Frame {
            output: "DP-1".into(),
            key,
            size: (1920, 1080),
            pixels: FramePixels::Dmabuf {
                fourcc: 0x3432_5258,
                modifier: 0x0300_0000_0060_6014,
                planes: vec![Plane {
                    fd: file.try_clone().unwrap().into(),
                    offset: 0,
                    stride: 7680,
                }],
            },
        };
        assert!(server.send(&frame(7)));
        assert!(server.send(&frame(7)));
        let events = client.read().unwrap();
        assert_eq!(events.len(), 2);
        let FeedEvent::Frame(f) = &events[0] else {
            panic!("not a frame")
        };
        assert_eq!((f.key, f.size, f.fds.len()), (7, (1920, 1080), 1));
        assert_eq!(f.modifier, 0x0300_0000_0060_6014);
        assert_eq!(f.planes, vec![(0, 7680)]);
        let FeedEvent::Frame(f) = &events[1] else {
            panic!("not a frame")
        };
        assert!(f.fds.is_empty(), "known buffers come without fds");

        client.release("DP-1", 7);
        assert_eq!(server.poll().0, vec![("DP-1".to_string(), 7)]);
        let inputs = [
            PointerInput::Motion { x: 0.25, y: 0.75 },
            PointerInput::Button {
                button: 272,
                pressed: true,
            },
            PointerInput::Axis {
                vertical: true,
                value: -10.0,
            },
            PointerInput::Leave,
        ];
        for i in inputs {
            client.pointer("DP-1", i);
        }
        server.poll();
        let got: Vec<PointerInput> = server.take_pointer().into_iter().map(|(_, i)| i).collect();
        assert_eq!(got, inputs, "the lock's pointer reaches the engine");
        assert!(server.send(&frame(9)));
        drop(client);
        // (other tests fork: a child may hold the client's fd until its exec)
        let mut freed = Vec::new();
        for _ in 0..200 {
            let (f, changed) = server.poll();
            freed.extend(f);
            if changed {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!server.active());
        assert_eq!(
            freed,
            vec![("DP-1".to_string(), 9)],
            "a closed lock frees what it held"
        );
        drop(server);
        assert!(!path.exists(), "socket removed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
