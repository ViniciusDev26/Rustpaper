//! Minimal libgbm FFI: allocates GPU buffers (dmabufs) the compositor can
//! copy captured frames into and then show, without a CPU round trip.
//!
//! With wl_shm the compositor read every 1080p frame back from the GPU and
//! uploaded it again: Hyprland went from 2.8% to 23% CPU at 60 fps
//! (measured 2026-11-22, Lonely Cat on DP-1). A dmabuf stays in VRAM.

use std::ffi::{c_int, c_void};
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;

#[repr(C)]
struct GbmDevice(c_void);
#[repr(C)]
struct GbmBo(c_void);

#[link(name = "gbm")]
unsafe extern "C" {
    fn gbm_create_device(fd: c_int) -> *mut GbmDevice;
    fn gbm_bo_create_with_modifiers(
        dev: *mut GbmDevice,
        width: u32,
        height: u32,
        format: u32,
        modifiers: *const u64,
        count: u32,
    ) -> *mut GbmBo;
    fn gbm_bo_create(
        dev: *mut GbmDevice,
        width: u32,
        height: u32,
        format: u32,
        flags: u32,
    ) -> *mut GbmBo;
    fn gbm_bo_get_plane_count(bo: *mut GbmBo) -> c_int;
    fn gbm_bo_get_fd_for_plane(bo: *mut GbmBo, plane: c_int) -> c_int;
    fn gbm_bo_get_stride_for_plane(bo: *mut GbmBo, plane: c_int) -> u32;
    fn gbm_bo_get_offset(bo: *mut GbmBo, plane: c_int) -> u32;
    fn gbm_bo_get_modifier(bo: *mut GbmBo) -> u64;
    fn gbm_bo_destroy(bo: *mut GbmBo);
}

/// GBM_BO_USE_RENDERING: the compositor renders (copies) into it.
const USE_RENDERING: u32 = 1 << 2;
/// DRM_FORMAT_MOD_INVALID: "implicit modifier" in the dmabuf protocol.
pub const MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
/// DRM_FORMAT_MOD_LINEAR.
const MOD_LINEAR: u64 = 0;

pub struct Device {
    _file: File,
    dev: *mut GbmDevice,
}

pub struct Plane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

pub struct Bo {
    bo: *mut GbmBo,
    pub modifier: u64,
    pub planes: Vec<Plane>,
}

impl Drop for Bo {
    fn drop(&mut self) {
        // SAFETY: bo came from gbm_bo_create*, destroyed once.
        unsafe { gbm_bo_destroy(self.bo) }
    }
}

impl Device {
    /// Opens the render node of the DRM device the compositor named
    /// (`dmabuf_device` is a dev_t, usually of the primary node `cardN`:
    /// its render node is the `renderD*` beside it in sysfs).
    pub fn open(dev: u64) -> std::io::Result<Self> {
        let (maj, min) = (libc::major(dev), libc::minor(dev));
        let mut path = None;
        if let Ok(rd) = std::fs::read_dir(format!("/sys/dev/char/{maj}:{min}/device/drm")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with("renderD") {
                    path = Some(format!("/dev/dri/{n}"));
                }
            }
        }
        // already a render node, or sysfs had none: match by st_rdev
        let path = path.or_else(|| {
            std::fs::read_dir("/dev/dri").ok()?.flatten().find_map(|e| {
                (e.metadata().ok()?.rdev() == dev).then(|| e.path().to_string_lossy().into_owned())
            })
        });
        let path =
            path.ok_or_else(|| std::io::Error::other(format!("no DRM node for {maj}:{min}")))?;
        let file = File::options().read(true).write(true).open(&path)?;
        // SAFETY: valid fd kept open for the device's lifetime (_file).
        let dev = unsafe { gbm_create_device(file.as_raw_fd()) };
        if dev.is_null() {
            return Err(std::io::Error::other(format!(
                "gbm_create_device({path}) failed"
            )));
        }
        eprintln!("dmabuf: allocating on {path}");
        Ok(Device { _file: file, dev })
    }

    /// A buffer the compositor can copy into: with the compositor's
    /// explicit modifiers, else (when the compositor accepts them) an
    /// implicit-modifier or a linear buffer. NVIDIA's GBM refused the
    /// explicit list for some sizes (even 1920x1080, right after a burst of
    /// resizes) while simpler layouts could still work. Only layouts the
    /// compositor advertised are tried: importing another one is a
    /// protocol error that ends the connection. None when all fail (the
    /// caller falls back to shm).
    pub fn alloc(&self, w: u32, h: u32, fourcc: u32, modifiers: &[u64]) -> Option<Bo> {
        let explicit: Vec<u64> = modifiers
            .iter()
            .copied()
            .filter(|&m| m != MOD_INVALID)
            .collect();
        let implicit_ok = explicit.is_empty() || modifiers.contains(&MOD_INVALID);
        // SAFETY: plain allocation calls; null checked.
        let (bo, modifier_known) = unsafe {
            let mut bo = std::ptr::null_mut();
            if !explicit.is_empty() {
                bo = gbm_bo_create_with_modifiers(
                    self.dev,
                    w,
                    h,
                    fourcc,
                    explicit.as_ptr(),
                    explicit.len() as u32,
                );
            }
            if !bo.is_null() {
                (bo, true)
            } else if implicit_ok {
                (gbm_bo_create(self.dev, w, h, fourcc, USE_RENDERING), false)
            } else if explicit.contains(&MOD_LINEAR) {
                let linear = [MOD_LINEAR];
                (
                    gbm_bo_create_with_modifiers(self.dev, w, h, fourcc, linear.as_ptr(), 1),
                    true,
                )
            } else {
                (std::ptr::null_mut(), false)
            }
        };
        if bo.is_null() {
            return None;
        }
        // SAFETY: bo valid; each fd returned is a new fd we own.
        unsafe {
            let modifier = if modifier_known {
                gbm_bo_get_modifier(bo)
            } else {
                MOD_INVALID
            };
            let planes = (0..gbm_bo_get_plane_count(bo))
                .map(|p| Plane {
                    fd: OwnedFd::from_raw_fd(gbm_bo_get_fd_for_plane(bo, p)),
                    offset: gbm_bo_get_offset(bo, p),
                    stride: gbm_bo_get_stride_for_plane(bo, p),
                })
                .collect();
            Some(Bo {
                bo,
                modifier,
                planes,
            })
        }
    }
}
