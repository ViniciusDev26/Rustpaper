// Módulo `video`: decodifica um vídeo usando o ffmpeg como subprocesso. Uma
// thread lê frames RGBA crus do stdout do ffmpeg e guarda sempre o mais recente
// num slot compartilhado. O render lê esse slot quando quer atualizar a textura.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

// O frame mais recente + um contador de "geração" (incrementa a cada frame novo),
// pra o render saber se já viu este frame.
struct Frame {
    data: Vec<u8>,
    generation: u64,
}

pub struct Video {
    pub width: u32,
    pub height: u32,
    slot: Arc<Mutex<Frame>>,
    child: Mutex<Option<Child>>, // guardado só pra matar o ffmpeg no Drop
}

impl Video {
    pub fn open(path: &str) -> Self {
        let (width, height) = probe(path);
        let frame_size = (width * height * 4) as usize; // RGBA = 4 bytes/pixel

        let slot = Arc::new(Mutex::new(Frame {
            data: vec![0u8; frame_size], // começa preto até o 1º frame chegar
            generation: 0,
        }));

        // ffmpeg: -stream_loop -1 = repete pra sempre; -re = ritmo real (paceia
        // na fps do vídeo); saída = rawvideo RGBA no stdout.
        let mut child = Command::new("ffmpeg")
            .args([
                "-loglevel", "error",
                "-stream_loop", "-1",
                "-re",
                "-i", path,
                "-f", "rawvideo",
                "-pix_fmt", "rgba",
                "-",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("falha ao iniciar o ffmpeg (instalado?)");

        let mut stdout = child.stdout.take().unwrap();
        let slot_thread = Arc::clone(&slot);

        // Thread de decodificação: lê um frame inteiro por vez e atualiza o slot.
        thread::spawn(move || loop {
            let mut frame = vec![0u8; frame_size];
            // read_exact preenche o buffer inteiro (bloqueia até ter um frame completo).
            if stdout.read_exact(&mut frame).is_err() {
                break; // ffmpeg terminou ou o pipe quebrou
            }
            let mut s = slot_thread.lock().unwrap();
            s.data = frame;
            s.generation += 1;
        });

        Video {
            width,
            height,
            slot,
            child: Mutex::new(Some(child)),
        }
    }

    // Se houver um frame mais novo que `last_seen`, chama `f` com os bytes dele
    // (segurando o lock — sem cópia extra) e devolve a nova geração.
    pub fn upload_if_newer<F: FnOnce(&[u8])>(&self, last_seen: u64, f: F) -> Option<u64> {
        let s = self.slot.lock().unwrap();
        if s.generation > last_seen {
            f(&s.data);
            Some(s.generation)
        } else {
            None
        }
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
        }
    }
}

// Descobre largura/altura do vídeo rodando o ffprobe.
fn probe(path: &str) -> (u32, u32) {
    let out = Command::new("ffprobe")
        .args([
            "-v", "error",
            "-select_streams", "v:0",
            "-show_entries", "stream=width,height",
            "-of", "csv=p=0",
            path,
        ])
        .output()
        .expect("falha ao rodar ffprobe");
    let s = String::from_utf8_lossy(&out.stdout);
    let s = s.trim(); // "1920,1080"
    let mut it = s.split(',');
    let w = it.next().unwrap().trim().parse().expect("largura inválida");
    let h = it.next().unwrap().trim().parse().expect("altura inválida");
    (w, h)
}
