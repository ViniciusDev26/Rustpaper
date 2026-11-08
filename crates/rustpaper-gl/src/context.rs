// Headless OpenGL context through EGL.
//
// We never create a window: everything is drawn into framebuffer objects (FBOs)
// and read back (or, later, shared with the presenter). EGL lets us get a real
// GPU-backed GL context without any window system. A 1x1 pbuffer is used as the
// "current surface" because some drivers (NVIDIA included, depending on version)
// are picky about fully surfaceless contexts.

use glow::HasContext;
use khronos_egl as egl;

pub struct GlContext {
    pub gl: glow::Context,
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    context: egl::Context,
    surface: egl::Surface,
}

impl GlContext {
    /// Creates a desktop OpenGL 3.3 core context (what `#version 330 core` needs).
    pub fn new_headless() -> Result<GlContext, String> {
        let egl = egl::Instance::new(egl::Static);
        // Try every GPU EGL exposes (EGL_EXT_device_enumeration) and keep the
        // first one that is NOT a software rasterizer. The default display on a
        // hybrid/NVIDIA box often resolves to Mesa's llvmpipe (CPU rendering).
        let mut last_err = String::from("no EGL device produced a hardware context");
        for display in candidate_displays(&egl) {
            match Self::on_display(display) {
                Ok(ctx) if !ctx.is_software() => return Ok(ctx),
                Ok(ctx) => last_err = format!("only software rendering: {}", ctx.describe()),
                Err(e) => last_err = e,
            }
        }
        if std::env::var_os("RUSTPAPER_ALLOW_SOFTWARE_GL").is_some() {
            // SAFETY: DEFAULT_DISPLAY is the documented "let the driver choose" value.
            if let Some(d) = unsafe { egl.get_display(egl::DEFAULT_DISPLAY) } {
                return Self::on_display(d);
            }
        }
        Err(last_err)
    }

    fn is_software(&self) -> bool {
        let r = unsafe { self.gl.get_parameter_string(glow::RENDERER) }.to_lowercase();
        r.contains("llvmpipe") || r.contains("softpipe") || r.contains("swrast")
    }

    fn on_display(display: egl::Display) -> Result<GlContext, String> {
        // `Static` is a zero-sized handle to the linked libEGL; cheap to recreate.
        let egl = egl::Instance::new(egl::Static);
        egl.initialize(display)
            .map_err(|e| format!("eglInitialize: {e}"))?;
        egl.bind_api(egl::OPENGL_API)
            .map_err(|e| format!("eglBindAPI(OpenGL): {e}"))?;

        let config_attribs = [
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
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
            .ok_or("no EGL config with OpenGL + pbuffer support")?;

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
        let surface = egl
            .create_pbuffer_surface(display, config, &[egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE])
            .map_err(|e| format!("eglCreatePbufferSurface: {e}"))?;
        egl.make_current(display, Some(surface), Some(surface), Some(context))
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
            context,
            surface,
        })
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

// EGL_EXT_platform_device: one EGLDisplay per physical GPU.
const PLATFORM_DEVICE_EXT: egl::Enum = 0x313F;

fn candidate_displays(egl: &egl::Instance<egl::Static>) -> Vec<egl::Display> {
    type QueryDevices =
        unsafe extern "system" fn(i32, *mut *mut std::ffi::c_void, *mut i32) -> egl::Boolean;
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
        let _ = self.egl.destroy_surface(self.display, self.surface);
        let _ = self.egl.destroy_context(self.display, self.context);
    }
}
