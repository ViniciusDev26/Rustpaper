# Rustpaper (wallpaper-engine-rs)

Reimplementação (em Rust, do zero) de um "Wallpaper Engine" para Linux: um
**engine** que renderiza wallpapers (vídeo e cenas do Wallpaper Engine da Steam)
na GPU via Vulkan, exibidos no fundo do desktop (Wayland/KWin via wlr-layer-shell),
e um **selector** (app desktop Tauri) para navegar o catálogo e aplicar.

Projeto de aprendizado de Rust — priorize clareza e explicação didática ao mexer.

## Workspace (Cargo)

Monorepo com 3 crates em `crates/`:

- **`rustpaper-core`** — lógica pura dos formatos do WE (sem GPU/UI). Módulos:
  `project` (project.json), `pkg` (.pkg PKGV), `tex` (.tex → RGBA + `SpriteSheet`),
  `scene` (scene.json → material/textura/partículas + `MaterialInfo`),
  `particle` (particles/*.json), `layout` (cena INTEIRA → camadas com transform),
  `effects` (grafo de efeitos), `shader` (tradução do dialeto WE → GLSL).
  Deps: `serde`, `serde_json`, `lz4_flex`, `image` (só jpeg/png), `texpresso`.
  Dev-dep: `naga` (valida o GLSL/SPIR-V produzido nos testes).
- **`rustpaper-engine`** — o renderer (lib + binário **`rustpaper`**). Módulos:
  `main`, `wallpaper` (layer-shell + event loop), `gpu` (wgpu: surface, cover-scale,
  render por monitor), `compositor` (render multi-camada da cena),
  `program` (material WE compilado: vert+frag linkados + UBO + blend),
  `shader_compile` (translate → glslang → SPIR-V + reflexão via naga),
  `postprocess` (primitiva de passe de efeito), `video` (ffmpeg), `particles`.
  Shaders: `shader.wgsl` (blit final por monitor), `particle.wgsl` (sprites).
  Deps: `rustpaper-core`, `wgpu` 29 (feature `spirv`), `naga` 29 (feature `spv-in`),
  `serde_json`, `pollster`, `bytemuck`, `raw-window-handle` 0.6,
  `smithay-client-toolkit` 0.19, `wayland-client` 0.31,
  `wayland-backend` (feature `client_system`), `calloop` 0.14, `calloop-wayland-source` 0.4.
  Precisa do **glslangValidator** em runtime (compila os shaders do WE).
- **`rustpaper-selector`** — app desktop Tauri v2. Binário **`rustpaper-selector`**.
  Backend Rust (`main`, `settings`) + frontend web estático em `ui/`. Comandos:
  `list_wallpapers`, `apply`, `get_settings`, `set_autostart`. Deps: `tauri` 2
  (feature `protocol-asset`), `serde`, `serde_json`, `rustpaper-core`.

## Como buildar e rodar

**Tudo roda DENTRO do devcontainer `vini-dev`** (é lá que estão Rust, ffmpeg,
Vulkan, o socket Wayland e os mounts). O binário fica em `target/debug/` na raiz.

```fish
# entrar no container
docker exec -it -u vscode vini-dev fish
cd ~/projects/personal/wallpaper-engine-rs

cargo build            # workspace inteiro
cargo test             # ~32 testes (unitários inline + integração de shader/naga)

# rodar um wallpaper direto no engine (pasta de um item do Workshop):
./target/debug/rustpaper /home/vscode/wallpapers/<id>

# rodar o selector (galeria); ele spawna o engine ao clicar:
cargo run -p rustpaper-selector
```

- O engine precisa de `WAYLAND_DISPLAY=/tmp/wayland-0` (já setado no container).
- O selector **embute** no `main.rs` os envs de container (`GDK_BACKEND=x11`,
  `WEBKIT_DISABLE_COMPOSITING_MODE/DMABUF`) — sem eles o webview crasha ou fica
  em branco. Rodar via `docker exec` funciona porque `DISPLAY=:0` já vem no container.
- Rode as ferramentas de inspeção de formato com os examples do rustpaper-core:
  `cargo run -p rustpaper-core --example dump_pkg|dump_scene|dump_tex -- <arquivo>`.

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
  **Suportamos ARGB8888, RG88, R8, DXT1/3/5 (via texpresso) e free-image.** ARGB8888 (apesar do
  nome) já vem em ordem **RGBA** — NÃO trocar canais (o WE faz upload como
  GL_RGBA; trocar R↔B faz vermelho virar azul, invisível em grayscale). RG88 =
  luminância(R)+alpha(G) → expandido pra (R,R,R,G); R8 = máscara (R,R,R,R).
  Strings do .tex
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

## Pipeline de shaders do WE (tradução → GPU)

Os shaders do WE (`.vert`/`.frag` nos assets) NÃO são GLSL puro: são um dialeto
cross-compile (HLSL+GLSL) com `#include`, `#require`, combos (`#if`), macros estilo
HLSL (`mul`, `frac`, `saturate`, `CAST3`, `texSample2D`) e GLSL legado (`varying`,
`gl_FragColor`). Detalhes completos em **`docs/SHADER_TRANSLATION.md`**. Pipeline:

    WE .vert/.frag
      │  rustpaper_core::shader::translate  → prelúdio (dialeto→GLSL) + includes + #require
      │                                + combos + SEPARA samplers (texture2D+sampler)
      ▼  GLSL 450 (Vulkan-flavored)
      │  glslangValidator -V -R --amb --aml --sdub WeGlobals 0 0   (subprocesso)
      ▼  SPIR-V Vulkan
      │  wgpu ShaderSource::SpirV  +  naga (reflexão do UBO/bindings)
      ▼  wgpu::RenderPipeline

- **naga glsl-in é fraco demais** (rejeita uniforms livres e qualifiers de sampler)
  → por isso compilamos via glslang. **naga spv-in rejeita samplers COMBINADOS** e o
  wgpu usa textura+sampler SEPARADOS → o `translate` reescreve `sampler2D` em
  `texture2D`+`sampler` e as macros remontam na amostragem. Bindings determinísticos:
  bloco `WeGlobals` = binding 0; textura `g_TextureN` = `2N+1`; sampler = `2N+2`.
- **UBO unificado entre estágios**: vert e frag declaram membros diferentes no mesmo
  bloco → compilar LINKADO (`glslang -l`) unifica o `WeGlobals` (mesmos offsets nos
  dois) → `Program` usa o vertex do WE (que a maioria dos efeitos precisa) com um UBO
  único. Ver `shader_compile::compile_linked`.
- `Program::build` captura erros de validação (error scope) e devolve `Err` em vez de
  dar panic — shaders com interface incompatível são pulados, não derrubam o engine.

## Compositor multi-camada (o render de cena atual)

Uma cena do WE é uma PILHA de camadas-imagem (a Ashe = 15 camadas: cabelo, luva,
capa, corpo...). O `rustpaper_core::layout::parse_layout` lê a cena inteira: projeção
ortográfica + cada objeto-imagem com transform (origin/scale/angles/size), material,
blend, alpha/cor/brilho e efeitos, em ordem de desenho.

- `rustpaper_engine::compositor::Compositor` pré-constrói cada camada (textura, `Program`, quad
  no espaço da cena, constants) e renderiza TUDO numa textura de conteúdo por frame:
  cada camada é um quad em espaço de cena (ortho → NDC, Y invertido) desenhado pelo
  shader real do WE, com **alpha/additive blend** e sRGB.
- No engine ao vivo (`gpu.rs`): `Source::SceneComposite(dir)` → o compositor preenche
  a textura de conteúdo a cada `tick` (com `g_Time` avançando); o desenho por-monitor
  (cover-scale, `shader.wgsl`) amostra essa textura. Reusa multi-monitor + timer.
- **sem forçar alpha opaco** (o caminho antigo forçava): a composição usa alpha real
  das texturas (é o que faz as camadas empilharem certo).

## Decisões de arquitetura (e o porquê)

- **Render dirigido por TIMER (calloop, ~30fps), não por frame callbacks do
  Wayland.** Frame callbacks param quando a superfície é ocluída → o wallpaper
  congelava. O timer desacopla e nunca congela. (`wallpaper::run` monta um
  `calloop::EventLoop` com `WaylandSource` + um `Timer`; `render_all` roda a cada tick.)
- **layer-shell (wlr), camada `Background`, uma layer surface por output**
  (multi-monitor). A surface do wgpu é criada dos ponteiros crus da wl_surface
  (`create_surface_unsafe`).
- **Cover na CPU** (`gpu::cover_scale`, testado): a textura de conteúdo (que o
  compositor preenche na resolução da cena) é escalada por monitor. O `content_norm`
  ainda serve pro caminho de vídeo/imagem única (recorte de padding).
- **Partículas**: simulação na CPU + render **instanciado** (uma draw call, N
  instâncias) por cima das camadas, em espaço de cena. Suporta sprite sheet
  (flipbook), escala do objeto, `turbulentvelocityrandom` + turbulência contínua,
  gravidade/drag, alphafade, oscillate, colorchange. Aditivo é amortecido (sem HDR).
- **NOTA**: o caminho antigo de 1 textura (`Source::Scene` + `scene_source` em
  wallpaper.rs, `#[allow(dead_code)]`) foi substituído pelo compositor. Mantido só
  como referência do modelo de partículas isolado.
- **engine e selector são DECOPLADOS**: o selector spawna o engine como processo
  filho (mata o anterior ao aplicar). Persistência do último wallpaper +
  autostart via arquivos XDG (state.json + .desktop).

## Estado de fidelidade (meta: vídeo + scenes fiéis — layers, partículas, animações)

Sem áudio/widgets no escopo. Onde estamos vs. a meta:

- **Vídeo**: ✅ fiel (loop, cover, multi-monitor).
- **Layers**: ✅ fiel — composição multi-camada com transform/blend/alpha/sRGB.
  Personagens que eram invisíveis (Ashe, GhostBlade, cyber girl) renderizam.
- **Partículas**: 🟡 aproximação boa (sprite sheet, escala, dispersão), mas o
  simulador é simples: faltam operadores (control points/attract, vortex, turbulência
  de campo real), formas de emissor (box/line), e HDR pro aditivo. Só `sprite`
  (spritetrail e afins pulados). Aditivo amortecido (×0.35) pra não estourar em branco.
- **Efeitos/animações**: ❌ **DESLIGADOS** — é a maior lacuna (por isso cenas com
  animação de efeito ficam estáticas). O scaffolding existe (`postprocess::Pass`,
  `compositor::draw_effect`/`composite`, `effects::parse_effect`), mas está gated:
  aplicar a cadeia parcial/quebrada destrói a imagem. Cenas de áudio/puppet (bones)
  renderizam pretas (fora de escopo por ora).

## Limitações e pegadinhas conhecidas

- **DXT decodificado** (DXT1/3/5 via texpresso) — fundos e sprites DXT funcionam.
- **Um engine por vez.** Dois engines na camada Background competem → flicker. O
  `apply` do selector mata o anterior; launches MANUAIS (via `docker exec`) podem
  acumular. Matar: `docker exec vini-dev pkill -9 -f "target/debug/rustpaper "`
  (nota o espaço no fim — evita casar também `rustpaper-selector`, que começa
  com o mesmo prefixo; o pkill do host e o do container veem PID namespaces
  diferentes — use o do container).
- **Concorrência com o `linux-wallpaperengine`**: se o autostart antigo dele subir,
  compete na camada Background → flicker. Deve ser desabilitado.
- **Autostart** dispara no login do **HOST**; hoje os binários/arquivos ficam no
  container (não é sessão de login). Só vale de verdade após um passo de
  **distribuição** (binários instalados no host + .desktop no ~/.config/autostart do host).
- **`pkill -f "target/debug/..."` mata o próprio shell do `docker exec`** (o comando
  casa o padrão) → use `pkill -f "debug/rustpaper "` (com o espaço, pra não pegar
  `rustpaper-selector` também) num exec SEPARADO do launch, ou faça kill e launch
  em execs distintos.
- **Verificação ao vivo**: rodar o engine detached (`setsid ... </dev/null & disown`),
  esperar, e `spectacle -b -n -f -o <png>` no HOST com o env da sessão Plasma (puxado
  de `/proc/<plasmashell>/environ`). Pra ver partículas/detalhes, recortar+ampliar com
  `ffmpeg -i shot.png -vf crop=W:H:X:Y out.png` (a tela cheia é 2 monitores lado a lado).

## Convenções

- **Testes**: unitários inline (`#[cfg(test)] mod tests`), lógica pura (parsers,
  cover, layout de uniforms). GPU/Wayland/UI verificados VISUALMENTE (não dá pra
  unit-testar). Ao capturar a tela pra verificar, use `spectacle -b -n -f` no host
  com o env da sessão Plasma.
- **Commits em INGLÊS** (o resto da conversa pode ser em português).
- Ao mexer na API do `wgpu`/SCTK, **cheque a fonte** no cargo registry antes de
  chutar — as APIs mudam bastante entre versões (já apanhamos várias vezes).

## Roadmap / frentes abertas

**North Star**: replicar fielmente o comportamento do WE para wallpapers de **vídeo**
e **scene** — layers, partículas e animações/efeitos. SEM áudio, música ou widgets
(horário etc.).

Ordenado por impacto de fidelidade:

1. **Pipeline de efeitos** (MAIOR lacuna — destrava a animação das cenas). Religar e
   consertar o caminho de efeitos por camada: renderizar a camada num FBO, aplicar a
   cadeia de passes (cada um amostra o quadro anterior), com FBOs nomeados + multi-pass
   pros complexos (godrays, blur). Resolver o mismatch de interface vertex↔fragment que
   trava vários shaders de efeito (usar o vertex do WE via `compile_linked`), o
   encadeamento por `bind`/`fbos` do `effect.json`, e os `combos`/`constantshadervalues`
   por passe (parsers já existem em `effects`). Começar por efeitos de 1 passe (tint,
   filmgrain, hue), depois multi-FBO.
2. **Fidelidade de partículas**: completar o simulador — operadores restantes
   (controlpoint/attract, vortex, turbulência de campo), formas de emissor (box/line),
   spritetrail, e um aditivo com tonemap em vez do amortecimento fixo.
3. **Animações de material** (g_Time nos shaders de camada): já rodam parcialmente;
   validar/expandir (scroll, pulse, etc. embutidos no material).

Infra/produto (menor prioridade): empacotamento/distribuição (AppImage/deb + instalar
no host, desabilitando o linux-wallpaperengine) · filtros/busca e "wallpaper atual" na
galeria do selector.

**Como retomar o pipeline de efeitos** (frente 1): ver `compositor.rs`
(`draw_effect`/`composite` já existem, gated), `postprocess::Pass`, `effects.rs`
(`parse_effect` dá passes/fbos/binds), e o exemplo `render_effect` (prova tint 1-passe
offscreen). O `PARTICLES_ENABLED`/gating de efeitos mostra o padrão de "ligar por peça
quando estiver fiel".
