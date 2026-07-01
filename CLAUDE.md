# wallpaper-engine-rs

Reimplementação (em Rust, do zero) de um "Wallpaper Engine" para Linux: um
**engine** que renderiza wallpapers (vídeo e cenas do Wallpaper Engine da Steam)
na GPU via Vulkan, exibidos no fundo do desktop (Wayland/KWin via wlr-layer-shell),
e um **selector** (app desktop Tauri) para navegar o catálogo e aplicar.

Projeto de aprendizado de Rust — priorize clareza e explicação didática ao mexer.

## Workspace (Cargo)

Monorepo com 3 crates em `crates/`:

- **`we-core`** — lógica pura dos formatos do WE (sem GPU/UI). Módulos:
  `project` (project.json), `pkg` (.pkg PKGV), `tex` (.tex → RGBA),
  `scene` (scene.json → texturas + partículas), `particle` (particles/*.json).
  Deps: `serde`, `serde_json`, `lz4_flex`, `image` (só jpeg/png).
- **`engine`** — o renderer. Binário **`wallpaper-engine-rs`**. Módulos:
  `main`, `wallpaper` (layer-shell + event loop), `gpu` (wgpu: pipeline, textura,
  uniforms, render), `video` (decodifica vídeo via ffmpeg), `particles` (sim + render).
  Shaders: `shader.wgsl` (fundo), `particle.wgsl` (sprites). Deps: `we-core`,
  `wgpu` 29, `pollster`, `bytemuck`, `raw-window-handle` 0.6,
  `smithay-client-toolkit` 0.19, `wayland-client` 0.31,
  `wayland-backend` (feature `client_system`), `calloop` 0.14, `calloop-wayland-source` 0.4.
- **`selector`** — app desktop Tauri v2. Binário **`wallpaper-selector`**.
  Backend Rust (`main`, `settings`) + frontend web estático em `ui/`. Comandos:
  `list_wallpapers`, `apply`, `get_settings`, `set_autostart`. Deps: `tauri` 2
  (feature `protocol-asset`), `serde`, `serde_json`, `we-core`.

## Como buildar e rodar

**Tudo roda DENTRO do devcontainer `vini-dev`** (é lá que estão Rust, ffmpeg,
Vulkan, o socket Wayland e os mounts). O binário fica em `target/debug/` na raiz.

```fish
# entrar no container
docker exec -it -u vscode vini-dev fish
cd ~/projects/personal/wallpaper-engine-rs

cargo build            # workspace inteiro
cargo test             # 24 testes (unitários, inline)

# rodar um wallpaper direto no engine (pasta de um item do Workshop):
./target/debug/wallpaper-engine-rs /home/vscode/wallpapers/<id>

# rodar o selector (galeria); ele spawna o engine ao clicar:
cargo run -p selector
```

- O engine precisa de `WAYLAND_DISPLAY=/tmp/wayland-0` (já setado no container).
- O selector **embute** no `main.rs` os envs de container (`GDK_BACKEND=x11`,
  `WEBKIT_DISABLE_COMPOSITING_MODE/DMABUF`) — sem eles o webview crasha ou fica
  em branco. Rodar via `docker exec` funciona porque `DISPLAY=:0` já vem no container.
- Rode as ferramentas de inspeção de formato com os examples do we-core:
  `cargo run -p we-core --example dump_pkg|dump_scene|dump_tex -- <arquivo>`.

## Devcontainer (repo `~/.config/devcontainer`)

O `up.sh` monta e configura (é preciso recriar o container com o **env da sessão
gráfica** — DISPLAY/WAYLAND_DISPLAY/XAUTHORITY; num login SSH puro eles não existem,
puxe de `/proc/<plasmashell>/environ`):

- **Workshop** do WE → `/home/vscode/wallpapers` (read-only). Contém as pastas
  `<id>/` com `project.json` + assets/pkg de cada wallpaper.
- **Assets base** do WE → `/home/vscode/we-assets` (read-only). Texturas/materiais
  compartilhados (ex.: sprites de partícula `materials/particle/*.tex`) que as
  cenas referenciam FORA do pkg.
- **Socket Wayland** do compositor → `/tmp/wayland-0` (habilita layer-shell).
- `NVIDIA_DRIVER_CAPABILITIES=all` → o nvidia-container-toolkit injeta o ICD do
  Vulkan (senão o wgpu não acha a GPU pelo Vulkan).

## Formatos do Wallpaper Engine (conhecimento acumulado)

- **project.json**: `type` (`video` | `scene` | `web`, casing VARIA), `file`
  (arquivo principal), `title`. Wallpaper de "imagem" não existe como tipo — é `scene`.
- **.pkg** (`scene.pkg`, header `PKGV0001`): sized-string de versão + `u32`
  filesCount + entradas (nome sized-string, `u32` offset, `u32` length) + blob de
  dados a partir de `base_offset`. Little-endian.
- **.tex** (`TEXV0005`/`TEXI0001`): header com `format` (0=ARGB8888, 4=DXT5,
  6=DXT3, 7=DXT1, ...), flags, dims. Container `TEXB0001..0004` (LZ4 quando
  `compression==1`; TEXB0003/4 podem ter imagem **free-image** JPEG/PNG embutida).
  **Suportamos ARGB8888 (bytes BGRA→RGBA) e free-image; DXT NÃO.** Strings do .tex
  são null-terminated (diferente do .pkg, que é length-prefixed).
- **Cadeia da cena**: `scene.json` → objeto com `image` → `models/x.json`
  (`material`) → `materials/x.json` (`passes[0].textures[0]` = nome) →
  `materials/<nome>.tex`. A textura tem padding pra potência de 2 (ex.: buffer
  2048x2048, conteúdo 1920x1080) — usamos `content_norm` no shader pra recortar.
- **Partículas**: objetos com `particle` → `particles/*.json`. Campos: `maxcount`,
  `emitter` (rate/origin/distance), `initializer` (lifetime/size/velocity/**color**),
  `operator` (movement gravity/drag, alphafade, oscillateposition, colorchange,
  controlpointattract, turbulence, ...), `renderer` (`sprite` | `spritetrail`),
  `material` → sprite (geralmente nos assets base). **Convenções de cor
  inconsistentes**: `colorrandom` usa 0-255; `colorchange` usa 0-1. `colorrandom`
  interpola numa LINHA (um `t` único) — sortear por-canal dá arco-íris (bug).

## Decisões de arquitetura (e o porquê)

- **Render dirigido por TIMER (calloop, ~30fps), não por frame callbacks do
  Wayland.** Frame callbacks param quando a superfície é ocluída → o wallpaper
  congelava. O timer desacopla e nunca congela. (`wallpaper::run` monta um
  `calloop::EventLoop` com `WaylandSource` + um `Timer`; `render_all` roda a cada tick.)
- **layer-shell (wlr), camada `Background`, uma layer surface por output**
  (multi-monitor). A surface do wgpu é criada dos ponteiros crus da wl_surface
  (`create_surface_unsafe`).
- **Alpha forçado opaco no fragment shader** — texturas com alpha < 1 deixavam a
  surface translúcida e o compositor "piscava" ao compor com o fundo.
- **Cover na CPU** (`gpu::cover_scale`, testado) + recorte de conteúdo pra ignorar
  o padding da textura.
- **Partículas**: simulação na CPU + render **instanciado** (uma draw call, N
  instâncias) com alpha blend, por cima do fundo. Operadores implementados:
  movement (gravity+drag), alphafade, oscillateposition, colorchange.
- **engine e selector são DECOPLADOS**: o selector spawna o engine como processo
  filho (mata o anterior ao aplicar). Persistência do último wallpaper +
  autostart via arquivos XDG (state.json + .desktop).

## Limitações e pegadinhas conhecidas

- **DXT não é decodificado** — cenas cujo fundo/texturas são DXT falham
  ("formato N não suportado"). Maior ganho de cobertura futuro seria implementar DXT.
- **Só renderizamos partículas com `renderer == "sprite"`** (degradação graciosa).
  `spritetrail` e afins são pulados (log informa quantos). Os sistemas densos de
  algumas cenas (ex.: Zoro) são **efeitos interativos de MOUSE**
  (`controlpointattract` num control point `locktopointer`) — um wallpaper de fundo
  não recebe ponteiro, então não faz sentido implementá-los.
- **Um engine por vez.** Dois engines na camada Background competem → flicker. O
  `apply` do selector mata o anterior; launches MANUAIS (via `docker exec`) podem
  acumular. Matar: `docker exec vini-dev pkill -9 -f target/debug/wallpaper-engine-rs`
  (o pkill do host e o do container veem PID namespaces diferentes — use o do container).
- **Concorrência com o `linux-wallpaperengine`**: se o autostart antigo dele subir,
  compete na camada Background → flicker. Deve ser desabilitado.
- **Autostart** dispara no login do **HOST**; hoje os binários/arquivos ficam no
  container (não é sessão de login). Só vale de verdade após um passo de
  **distribuição** (binários instalados no host + .desktop no ~/.config/autostart do host).
- **Sprite único por cena** e **coordenadas de partícula assumem cena ≈ tela 16:9**
  (monitores do dev são 16:9). Aspectos diferentes desalinham levemente.
- **Sprite sem padding** assumido (halo é 64x64); sprites com padding amostrariam borda.

## Convenções

- **Testes**: unitários inline (`#[cfg(test)] mod tests`), lógica pura (parsers,
  cover, layout de uniforms). GPU/Wayland/UI verificados VISUALMENTE (não dá pra
  unit-testar). Ao capturar a tela pra verificar, use `spectacle -b -n -f` no host
  com o env da sessão Plasma.
- **Commits em INGLÊS** (o resto da conversa pode ser em português).
- Ao mexer na API do `wgpu`/SCTK, **cheque a fonte** no cargo registry antes de
  chutar — as APIs mudam bastante entre versões (já apanhamos várias vezes).

## Roadmap / frentes abertas

DXT no decoder (amplia cobertura) · múltiplas camadas de imagem por cena ·
filtros/busca na galeria · empacotamento/distribuição (AppImage/deb + instalar no
host, desabilitando o linux-wallpaperengine) · destacar o wallpaper atual na galeria.
