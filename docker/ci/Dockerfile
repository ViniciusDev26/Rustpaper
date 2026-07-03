# Imagem de build da CI do Rustpaper: Ubuntu 24.04 + toolchain Rust + as deps de
# sistema do workspace inteiro (rustpaper-engine precisa de wayland/xkbcommon +
# glslangValidator pra compilar os shaders do WE nos testes; rustpaper-selector é
# um app Tauri e precisa dos -dev do webkit2gtk/gtk). Nada aqui precisa de GPU ou
# de um compositor Wayland rodando: wgpu (Vulkan) e o wayland-client resolvem o
# driver/compositor via dlopen em tempo de EXECUÇÃO, não em tempo de build.
#
# Publicada em ghcr.io pelo job `image` de .github/workflows/ci.yml (que roda antes
# dos outros jobs, então a imagem está sempre em dia com este arquivo). Lista de
# pacotes validada rodando o build/test/clippy/fmt do zero num container limpo.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        pkg-config \
        libwayland-dev \
        libxkbcommon-dev \
        glslang-tools \
        libwebkit2gtk-4.1-dev \
        libgtk-3-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libssl-dev \
        libxdo-dev \
    && rm -rf /var/lib/apt/lists/*

# --profile minimal: sem docs/extras, só o essencial pra compilar em CI.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --profile minimal --default-toolchain stable \
        --component rustfmt --component clippy \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

RUN rustc --version && cargo --version && cargo clippy --version && cargo fmt --version \
    && glslangValidator --version
