# Rustpaper's CI build image: Ubuntu 24.04 + Rust toolchain + the system deps
# of the whole workspace (rustpaper-engine needs wayland + libgbm, which it
# links to allocate dmabufs; rustpaper-selector is a Tauri app and needs the
# webkit2gtk/gtk -dev packages). Nothing here needs a GPU or a running
# Wayland compositor.
#
# Published to ghcr.io by the `image` job of .github/workflows/ci.yml (which
# runs before the other jobs, so the image always matches this file). Package
# list validated by running build/test/clippy/fmt from scratch in a clean
# container.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

# build-essential: the linker (`cc`) and a C compiler. --no-install-recommends
# no longer pulls them in through the -dev packages.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        pkg-config \
        libwayland-dev \
        libgbm-dev \
        libegl-dev \
        libxkbcommon-dev \
        libwebkit2gtk-4.1-dev \
        libgtk-3-dev \
        libayatana-appindicator3-dev \
        librsvg2-dev \
        libssl-dev \
        libxdo-dev \
    && rm -rf /var/lib/apt/lists/*

# --profile minimal: no docs/extras, just what CI needs to compile.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
        sh -s -- -y --profile minimal --default-toolchain stable \
        --component rustfmt --component clippy \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# Node.js 22 (NodeSource): rustpaper-selector's build.rs builds its React UI
# with npm/Vite (Ubuntu 24.04's own nodejs is 18, too old for Vite/Tailwind 4).
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/*

RUN rustc --version && cargo --version && cargo clippy --version && cargo fmt --version \
    && node --version && npm --version
