// Esqueleto do selector (app desktop Tauri). Por ora só abre uma janela com o
// frontend estático em ui/. Os "comandos" (listar wallpapers, aplicar) que usam
// o we-core virão depois.

fn main() {
    setup_linux_env();

    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("erro ao rodar o app Tauri");
}

// Acomodações pra rodar o webview (webkit2gtk) dentro do devcontainer:
// - GDK_BACKEND=x11: o backend Wayland dá "Protocol error" pelo socket montado;
//   via X11 (Xwayland) funciona.
// - WEBKIT_DISABLE_*: desliga compositing/DMABUF por GPU, que falham no container
//   (libEGL: failed to create dri2 screen) e deixariam a janela em branco.
// Só setamos o que ainda não estiver definido (respeita override do usuário).
// TODO: rever isto ao empacotar pra distribuição (num desktop Wayland real,
// preferir o backend nativo).
fn setup_linux_env() {
    let defaults = [
        ("GDK_BACKEND", "x11"),
        ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
    ];
    for (key, val) in defaults {
        if std::env::var_os(key).is_none() {
            // set_var é unsafe na edition 2024 (segurança de threads); aqui é seguro
            // pois rodamos no início do main, antes de qualquer thread/GTK subir.
            unsafe { std::env::set_var(key, val) };
        }
    }
}
