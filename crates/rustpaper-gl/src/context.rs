// OpenGL context through EGL, in two flavours:
//
// - headless (`new_headless`): no window system at all. Everything is drawn
//   into framebuffer objects and read back — the CLI / reference harness. A 1x1
//   pbuffer is the "current surface" because some drivers (NVIDIA included,
//   depending on version) are picky about fully surfaceless contexts.
// - Wayland (`new_wayland`): same GL context, but on the compositor's display so
//   it can own one window surface per monitor (the live wallpaper). The scene is
//   rendered once into an FBO and blitted to every monitor's surface.

use std::ffi::c_void;

use glow::HasContext;
use khronos_egl as egl;

pub use egl::Surface;

pub struct GlContext {
    pub gl: glow::Context,
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    pbuffer: Option<egl::Surface>,
}

// EGL_EXT_platform_device: one EGLDisplay per physical GPU.
const PLATFORM_DEVICE_EXT: egl::Enum = 0x313F;
// EGL_KHR_platform_wayland
const PLATFORM_WAYLAND_KHR: egl::Enum = 0x31D8;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Headless,
    Window,
}

impl GlContext {
    /// Desktop OpenGL 3.3 core context (what `#version 330 core` needs) with no
    /// window system, on the first hardware GPU.
    pub fn new_headless() -> Result<GlContext, String> {
        let egl = egl::Instance::new(egl::Static);
        // Try every GPU EGL exposes (EGL_EXT_device_enumeration) and keep the
        // first one that is NOT a software rasterizer. The default display on a
        // hybrid/NVIDIA box often resolves to Mesa's llvmpipe (CPU rendering).
        let mut last_err = String::from("no EGL device produced a hardware context");
        for display in candidate_displays(&egl) {
            match Self::on_display(display, Kind::Headless) {
                Ok(ctx) if !ctx.is_software() => return Ok(ctx),
                Ok(ctx) => last_err = format!("only software rendering: {}", ctx.describe()),
                Err(e) => last_err = e,
            }
        }
        if std::env::var_os("RUSTPAPER_ALLOW_SOFTWARE_GL").is_some() {
            // SAFETY: DEFAULT_DISPLAY is the documented "let the driver choose" value.
            if let Some(d) = unsafe { egl.get_display(egl::DEFAULT_DISPLAY) } {
                return Self::on_display(d, Kind::Headless);
            }
        }
        Err(last_err)
    }

    /// Context on a Wayland connection (`wl_display*` from wayland-client's
    /// `backend().display_ptr()`), able to create window surfaces.
    ///
    /// # Safety
    /// `wl_display` must be a live `wl_display*` that outlives the context.
    pub unsafe fn new_wayland(wl_display: *mut c_void) -> Result<GlContext, String> {
        let egl = egl::Instance::new(egl::Static);
        // SAFETY: caller guarantees the pointer is a live wl_display.
        let display = unsafe {
            egl.get_platform_display(PLATFORM_WAYLAND_KHR, wl_display, &[egl::ATTRIB_NONE])
        }
        .map_err(|e| format!("eglGetPlatformDisplay(wayland): {e}"))?;
        Self::on_display(display, Kind::Window)
    }

    fn is_software(&self) -> bool {
        let r = unsafe { self.gl.get_parameter_string(glow::RENDERER) }.to_lowercase();
        r.contains("llvmpipe") || r.contains("softpipe") || r.contains("swrast")
    }

    fn on_display(display: egl::Display, kind: Kind) -> Result<GlContext, String> {
        // `Static` is a zero-sized handle to the linked libEGL; cheap to recreate.
        let egl = egl::Instance::new(egl::Static);
        egl.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_API)
            .map_err(|e| format!("eglBindAPI(OpenGL): {e}"))?;

        let surface_type = match kind {
            Kind::Headless => egl::PBUFFER_BIT,
            Kind::Window => egl::WINDOW_BIT,
        };
        let config_attribs = [
            egl::SURFACE_TYPE,
            surface_type,
            egl::RENDERABLE_TYPE,
            egl::OPENGL_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &config_attribs)
            .map_err(|e| format!("eglChooseConfig: {e}"))?
            .ok_or("no EGL config with OpenGL support for this surface type")?;

        let context_attribs = [
            egl::CONTEXT_MAJOR_VERSION,
            3,
            egl::CONTEXT_MINOR_VERSION,
            3,
            egl::CONTEXT_OPENGL_PROFILE_MASK,
            egl::CONTEXT_OPENGL_CORE_PROFILE_BIT,
            egl::NONE,
        ];
        let context = egl
            .create_context(display, config, None, &context_attribs)
            .map_err(|e| format!("eglCreateContext: {e}"))?;

        let pbuffer = match kind {
            Kind::Headless => Some(
                egl.create_pbuffer_surface(
                    display,
                    config,
                    &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE],
                )
                .map_err(|e| format!("eglCreatePbufferSurface: {e}"))?,
            ),
            // No window exists yet: start surfaceless (EGL_KHR_surfaceless_context)
            // so resources (scene, textures, shaders) can be created right away.
            Kind::Window => None,
        };
        egl.make_current(display, pbuffer, pbuffer, Some(context))
            .map_err(|e| format!("eglMakeCurrent: {e}"))?;

        // SAFETY: the context is current on this thread; glow only stores the
        // function pointers EGL hands back.
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |p| p as *const _)
            })
        };
        Ok(GlContext {
            gl,
            egl,
            display,
            config,
            context,
            pbuffer,
        })
    }

    /// Window surface for a native window (a `wl_egl_window*` on Wayland).
    ///
    /// # Safety
    /// `native_window` must be valid and outlive the returned surface.
    pub unsafe fn create_window_surface(
        &self,
        native_window: *mut c_void,
    ) -> Result<Surface, String> {
        // SAFETY: forwarded from the caller.
        unsafe {
            self.egl
                .create_window_surface(self.display, self.config, native_window, None)
        }
        .map_err(|e| format!("eglCreateWindowSurface: {e}"))
    }

    pub fn destroy_surface(&self, surface: Surface) {
        let _ = self.egl.destroy_surface(self.display, surface);
    }

    /// Makes `surface` the draw target of the (single) context. With `None`, the
    /// context stays current without a surface (FBO-only rendering).
    pub fn make_current(&self, surface: Option<Surface>) -> Result<(), String> {
        let s = surface.or(self.pbuffer);
        self.egl
            .make_current(self.display, s, s, Some(self.context))
            .map_err(|e| format!("eglMakeCurrent: {e}"))
    }

    /// Don't wait for vblank on swap: the wallpaper is paced by its own timer,
    /// and a hidden surface must never block the render loop.
    pub fn disable_vsync(&self) {
        let _ = self.egl.swap_interval(self.display, 0);
    }

    pub fn swap(&self, surface: Surface) -> Result<(), String> {
        self.egl
            .swap_buffers(self.display, surface)
            .map_err(|e| format!("eglSwapBuffers: {e}"))
    }

    /// "NVIDIA Corporation / NVIDIA GeForce ... / 4.6.0 NVIDIA ..." — for logs.
    pub fn describe(&self) -> String {
        unsafe {
            format!(
                "{} / {} / {}",
                self.gl.get_parameter_string(glow::VENDOR),
                self.gl.get_parameter_string(glow::RENDERER),
                self.gl.get_parameter_string(glow::VERSION)
            )
        }
    }
}

fn candidate_displays(egl: &egl::Instance<egl::Static>) -> Vec<egl::Display> {
    type QueryDevices = unsafe extern "system" fn(i32, *mut *mut c_void, *mut i32) -> egl::Boolean;
    let mut out = Vec::new();
    if let Some(f) = egl.get_proc_address("eglQueryDevicesEXT") {
        // SAFETY: signature from the EGL_EXT_device_enumeration spec.
        let query: QueryDevices = unsafe { std::mem::transmute(f) };
        let mut devices = [std::ptr::null_mut(); 16];
        let mut n = 0;
        if unsafe { query(devices.len() as i32, devices.as_mut_ptr(), &mut n) } == egl::TRUE {
            for &dev in &devices[..n.max(0) as usize] {
                // SAFETY: `dev` came from eglQueryDevicesEXT just above.
                if let Ok(d) = unsafe {
                    egl.get_platform_display(PLATFORM_DEVICE_EXT, dev, &[egl::ATTRIB_NONE])
                } {
                    out.push(d);
                }
            }
        }
    }
    out
}

impl Drop for GlContext {
    fn drop(&mut self) {
        let _ = self.egl.make_current(self.display, None, None, None);
        if let Some(s) = self.pbuffer {
            let _ = self.egl.destroy_surface(self.display, s);
        }
        let _ = self.egl.destroy_context(self.display, self.context);
    }
}
