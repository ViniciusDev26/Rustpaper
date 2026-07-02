# Boss final: tradução de shaders WE → WGSL

Objetivo: renderizar cenas do Wallpaper Engine com fidelidade (materiais/efeitos
usam os shaders do WE). É o maior componente que falta — o grosso do
linux-wallpaperengine.

## O que os shaders do WE são (investigado)

GLSL **legado** (estilo GLSL ES 1.0). Ex.: `assets/shaders/genericimage2.frag`:
- `#include "common_pbr.h"`, `#include "common_blending.h"` — includes próprios
  (resolver a partir de `assets/shaders/` e `assets/shaders/base/`).
- `uniform sampler2D g_Texture0; // {"label":...,"combo":"NORMALMAP","default":...}`
  — uniforms anotados com JSON (defaults, ranges, combos, formato de textura).
- `#if COMBO` / `#ifndef VERSION` — **combos** são feature flags setadas por
  material (`passes[].combos` no material.json) e por presença de texturas.
- `varying vec2 v_TexCoord;`, `texture2D(...)`, provavelmente `gl_FragColor` —
  construções GLSL antigas.

## Por que o lwe tem vantagem

Ele usa **OpenGL**, então roda o GLSL do WE quase direto: só resolve includes +
injeta `#define`s dos combos + alguns patches. Nós usamos **wgpu (WGSL)**, então
precisamos de GLSL→WGSL.

## Pipeline PROVADO (compila ponta-a-ponta com genericimage2 real)

    WE .vert/.frag
      │  shader::translate  (we-core)
      ▼  resolve #include + prelúdio (dialeto→GLSL) + combos + SEPARA samplers
    GLSL 450 (Vulkan-flavored, samplers separados)
      │  glslangValidator -V -R --amb --aml --sdub WeGlobals 0 0 -S <stage>
      ▼
    SPIR-V Vulkan   (spirv-val limpo)
      │  naga spv-in + Validator   (== o que o wgpu faz)
      ▼
    naga::Module    → wgpu::ShaderSource::SpirV

Bindings resultantes (determinísticos, ver `shader::texture_bindings`):
bloco de uniforms livres `WeGlobals` = set 0 binding 0; textura `g_TextureN` =
binding `2N+1`; sampler `_smp_g_TextureN` = binding `2N+2`.

### Descobertas empíricas (por que este caminho, e não outro)

- **naga glsl-in é fraco demais**: rejeita `#version 330`, exige que uniforms
  livres estejam em blocos com binding, e NÃO implementa qualifiers de sampler
  (`uniform sampler2D` → `NotImplemented("variable qualifier")`). Descartado.
- **glslang aceita o dialeto** (com o prelúdio) e gera SPIR-V Vulkan limpo. Só que
  Vulkan proíbe uniforms livres → `-R` (regras relaxadas) + `--sdub` junta todos
  num bloco default nomeado.
- **naga spv-in rejeita samplers COMBINADOS** (o `sampler2D` estilo GL que o
  glslang emite: `InvalidId` no `OpLoad` da imagem-amostrada). Além disso, o modelo
  de bind group do wgpu é SEPARADO (textura + sampler distintos) — não existe
  sampler combinado. Por isso `translate` reescreve
  `uniform sampler2D g_TexN` → `uniform texture2D g_TexN` + `uniform sampler
  _smp_g_TexN`, e as macros `texSample2D/Lod` remontam `sampler2D(tex, smp)` na
  amostragem. glslang então emite imagem+sampler separados, que o naga spv-in
  ACEITA.

Provado por `crates/we-core/tests/naga_compile.rs` (genericimage2 vert+frag).

## Estado do render (offscreen, verificado por PNG)

- **Fundo real** renderiza pelo shader real do WE (`render_scene` example, cena Zoro).
- **Cadeia de efeitos** funciona (`render_effect`): fundo → efeito `tint` amostrando
  o quadro anterior. `postprocess::Pass` é a primitiva de passe reutilizável.
- **Reflexão** (`shader_compile::reflect`): lê do SPIR-V o layout do bloco de
  uniforms (offsets por nome) e os bindings de textura/sampler via naga.
- **Parsers** (we-core): `scene::MaterialInfo` (shader/combos/constants),
  `effects` (grafo de efeitos: objeto→effect.json→passes/fbos/binds + overrides),
  `shader::parse_params` (anotações `// {json}`: uniform↔material + defaults).

### UBO unificado entre estágios (o último bloqueio, resolvido)

A maioria dos efeitos precisa do VERTEX do WE (ex.: filmgrain calcula
`v_TexCoordNoise` a partir de `g_Time` no vertex). Mas vertex e fragment declaram
membros DIFERENTES no bloco `WeGlobals` — compilados separados, colidiriam no
binding 0. Solução: compilar vertex+fragment LINKADOS (`glslang -l`), que UNIFICA o
bloco default — os dois SPIR-V saem com o MESMO struct e os MESMOS offsets. Ver
`shader_compile::compile_linked` e o teste `engine/tests/linked_compile.rs`
(genericimage2: UBO unificado de 96 bytes, `g_ModelViewProjectionMatrix@0` +
`g_Brightness@88` visíveis nos dois estágios).

### O que falta pro render fiel de efeitos animados

Orquestração (sem incógnitas): usar `compile_linked` (vert+frag do WE) por passe,
montar o UBO unificado com `parse_params` (defaults) + constants (material/cena) +
builtins (`g_Time`, resoluções), resolver texturas de entrada do efeito (framebuffer
anterior via `bind`, mais texturas base tipo `util/noise`), e orquestrar
multi-passe/multi-FBO (blur, godrays). Integração ao vivo no `wallpaper.rs` por fim.

### Fases (para uma sessão futura, com contexto fresco)

1. **Resolver includes** — módulo que lê um shader do WE e expande `#include "x.h"`
   recursivamente a partir de `we-assets/shaders/` (+ `/base`). Pura lógica de
   texto → testável no we-core.
2. **Combos/defines** — ler `passes[].combos` do material + os combos implícitos
   (ex.: textura presente liga um combo). Montar a lista de `#define NOME valor`.
   As anotações `// {json}` dos uniforms dão os defaults dos combos.
3. **Modernizar o GLSL** pra o naga aceitar: `#version 450`, `varying`→`in/out`
   (por estágio), `attribute`→`in`, `texture2D`→`texture`, `gl_FragColor`→um
   `layout(location=0) out vec4`, remover/adaptar o que o naga não engole. Este é
   o passo mais chato e iterativo (o frontend GLSL do naga é exigente).
4. **Bindings/uniforms** — mapear os `g_*` (matrizes MVP, g_Time, g_Texture0..N,
   parâmetros de material) pra um uniform buffer + bind groups. Precisa casar com
   o que os vert/frag esperam (as `varying` conectam os dois estágios).
5. **Wire no engine** — em vez do nosso `shader.wgsl` fixo por cena, compilar o
   material da cena (shader + combos + texturas) e desenhar cada camada de imagem
   com seu material. Isso também destrava **múltiplas camadas** de imagem.
6. **Efeitos / FBO** — cadeia de efeitos (bloom, etc.) renderiza pra textura
   (FBO) e encadeia passes. HDR/tonemapping. É o que falta pras cenas "pesadas"
   não estourarem em branco (o problema atual das partículas additive).

### Riscos / realidade

- O frontend GLSL do naga não aceita tudo (ES 1.0 antigo, extensões). Fase 3 é
  tentativa-e-erro; alguns shaders podem não compilar sem patches específicos.
- Combos: a explosão de variantes (`#if`) é grande; começar só com os combos que
  as cenas-alvo usam (ex.: `genericimage2` sem PBR/lighting).
- Isso é trabalho de **semanas**. Estratégia: fazer UM material simples
  (`genericimage2`, sem combos) renderizar via naga primeiro (marco 1), depois
  crescer. Não tentar tudo de uma vez.

## Ponto de partida sugerido (marco 1)

Pegar `genericimage2.vert/.frag`, resolver includes, sem combos, modernizar o
mínimo, compilar via `ShaderSource::Glsl`, e desenhar a camada de fundo de uma
cena de imagem simples com ELE (em vez do nosso shader fixo). Se um quad texturado
aparecer via o shader do WE traduzido pelo naga, o caminho está provado.
