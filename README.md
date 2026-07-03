<p align="center">
  <img src="crates/rustpaper-selector/icons/icon.png" width="140" alt="Ícone do Rustpaper">
</p>

<h1 align="center">Rustpaper</h1>
<p align="center"><i>Wallpaper Engine, reconstruído do zero em Rust — pro seu Linux.</i></p>

<p align="center">
  <img alt="Rust" src="https://img.shields.io/badge/rust-stable-orange?logo=rust">
  <img alt="Platform" src="https://img.shields.io/badge/platform-linux%20%2F%20wayland-blue">
  <img alt="Status" src="https://img.shields.io/badge/status-beta-yellow">
  <img alt="License" src="https://img.shields.io/badge/license-TBD-lightgrey">
</p>

---

**Rustpaper** (repositório `wallpaper-engine-rs`) é uma reimplementação, em
Rust e do zero, de um **Wallpaper Engine para Linux**: um *engine* que
renderiza na GPU (Vulkan, via [wgpu](https://wgpu.rs)) wallpapers de vídeo e de
**cenas** do Wallpaper Engine (Steam) direto no fundo do desktop
(Wayland/KWin), e um *selector* (app desktop) pra navegar sua biblioteca e
aplicar um wallpaper com um clique.

Projeto pessoal de estudo de Rust e gráfica de baixo nível — não é um produto
oficial nem afiliado à Valve ou ao Wallpaper Engine.

---

## ⚠️ Requisito obrigatório: você precisa ter o Wallpaper Engine

Este projeto **não distribui nenhum conteúdo de wallpaper**. Ele apenas lê e
renderiza os arquivos (`project.json`, `.pkg`, `.tex`, `scene.json`, etc.) que
já existem na sua biblioteca local do **Wallpaper Engine, comprado na Steam**.

Ou seja, pra usar isso você precisa:

1. Ter o [Wallpaper Engine](https://store.steampowered.com/app/431960/Wallpaper_Engine/)
   comprado e instalado (via Steam);
2. Ter baixado/assinado os wallpapers desejados pelo Workshop dele — o que cria
   as pastas de conteúdo (`project.json` + assets) que este engine sabe ler.

Sem isso, não há nada pra este projeto renderizar.

---

## 🚧 Status do projeto: beta, sem releases

O projeto está em **desenvolvimento ativo e ainda não tem builds/binários pra
download**. Por enquanto, a única forma de usar é compilando o código-fonte
você mesmo (veja `CLAUDE.md` para o passo a passo de build, que hoje depende
de um devcontainer com Rust, Vulkan e `glslangValidator`).

A ideia é, futuramente, disponibilizar pacotes prontos (ex.: AppImage/deb)
pra instalar sem precisar compilar nada — mas isso só faz sentido depois que
a fidelidade de renderização (efeitos, partículas) estiver mais madura. Até lá,
**não existem releases** deste projeto.

---

## ✨ O que já funciona (fidelidade)

- **Vídeo**: fiel — loop, cover, multi-monitor.
- **Cenas (layers)**: fiel — composição multi-camada com transform, blend,
  alpha e sRGB corretos.
- **Partículas**: aproximação boa (sprite sheet, escala, dispersão), mas o
  simulador ainda é simples (faltam alguns operadores e formas de emissor).
- **Efeitos/animações de pós-processamento**: ainda desligados — é a maior
  lacuna hoje, então cenas que dependem de efeitos animados ficam estáticas.

Sem suporte a áudio, música ou widgets (relógio etc.) — fora do escopo.

---

## 🧩 Arquitetura (resumo)

Monorepo Cargo com 3 crates em `crates/`:

- **`rustpaper-core`** — parsing dos formatos do Wallpaper Engine (project, pkg, tex,
  scene, particles, layout, effects, shaders), sem depender de GPU/UI.
- **`rustpaper-engine`** — o renderer de verdade: binário `rustpaper`, usa
  wgpu/Vulkan + wlr-layer-shell (Wayland) pra desenhar o wallpaper no fundo
  do desktop.
- **`rustpaper-selector`** — app desktop (Tauri) pra navegar o catálogo de
  wallpapers já baixados e aplicar um deles (spawna o `rustpaper` como
  processo filho).

Detalhes de build, formatos suportados e decisões de arquitetura estão em
[`CLAUDE.md`](./CLAUDE.md).

---

## 📜 Aviso legal

Este é um projeto independente e não-oficial, feito para uso pessoal e fins
de aprendizado. Não tem qualquer afiliação com a Valve ou com os
desenvolvedores do Wallpaper Engine. Ele foi pensado para operar apenas sobre
conteúdo que você já obteve legitimamente através do Wallpaper Engine oficial.
