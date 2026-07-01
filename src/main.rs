// Declara os módulos do projeto. Cada `mod X;` faz o Rust carregar `src/X.rs`.
mod app;
mod gpu;

use crate::app::App;
use winit::event_loop::EventLoop;

fn main() {
    let event_loop = EventLoop::new().unwrap();
    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}
