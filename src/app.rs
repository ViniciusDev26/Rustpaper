// Módulo `app`: cuida da janela e dos eventos (winit). Não sabe desenhar — ele
// delega isso pro GpuState do módulo `gpu`.

use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

// Importa o GpuState do módulo vizinho. `crate` = raiz do projeto (src/).
use crate::gpu::GpuState;

#[derive(Default)]
pub struct App {
    // O estado da GPU só existe depois que a janela é criada (em `resumed`),
    // por isso Option: começa None, vira Some(GpuState) quando pronto.
    state: Option<GpuState>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let window = event_loop
            .create_window(Window::default_attributes().with_title("wallpaper-engine-rs"))
            .unwrap();
        self.state = Some(GpuState::new(Arc::new(window)));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // let-else: se ainda não há estado, sai cedo. Senão, `state` é o &mut.
        let Some(state) = self.state.as_mut() else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.resize(size.width, size.height),
            WindowEvent::RedrawRequested => {
                state.render();
                // Pede o próximo frame -> loop de render contínuo.
                state.request_redraw();
            }
            _ => {}
        }
    }
}
