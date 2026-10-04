FROM docker.io/devkitpro/devkita64:latest

# Install Nintendo Switch graphics, audio, and dev tools
RUN dkp-pacman -Syu --noconfirm \
    switch-mesa \
    switch-sdl2 \
    switch-tools \
    libnx \
    devkitA64 && \
    dkp-pacman -Scc --noconfirm

# Install Rust toolchain with Switch cross-compilation support
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:/opt/devkitpro/devkitA64/bin:/opt/devkitpro/tools/bin:$PATH \
    DEVKITPRO=/opt/devkitpro \
    DEVKITARM=/opt/devkitpro/devkitA64

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly && \
    rustup component add rust-src rust-std llvm-tools-preview

# Install cargo-nx and elf2nro for packaging .nro homebrew
RUN cargo install cargo-nx || true

WORKDIR /workspace
CMD ["/bin/bash"]
