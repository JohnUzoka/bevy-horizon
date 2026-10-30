FROM docker.io/devkitpro/devkita64:latest

# Install Nintendo Switch graphics, audio, and dev tools. switch-curl is libcurl with libnx's TLS
# backend (the console's SSL service and its trusted certificates), for games that make HTTPS
# requests (docs/porting-a-bevy-game.md, "Networking").
RUN dkp-pacman -Syu --noconfirm \
    switch-mesa \
    switch-sdl2 \
    switch-curl \
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

# The nightly is pinned to the one the documented builds used; --build-arg RUST_NIGHTLY=nightly
# takes the latest. A dated toolchain is renamed to plain "nightly": -Z build-std compiles std
# from the sysroot, so its path (/usr/local/rustup/toolchains/nightly-x86_64-unknown-linux-gnu)
# is in every binary's debug info, and keeping it lets a rebuild match an earlier one byte for byte.
ARG RUST_NIGHTLY=nightly-2026-10-04
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
        --default-toolchain "$RUST_NIGHTLY" --component rust-src,llvm-tools-preview && \
    if [ "$RUST_NIGHTLY" != nightly ]; then \
        mv "$RUSTUP_HOME/toolchains/$RUST_NIGHTLY-x86_64-unknown-linux-gnu" \
           "$RUSTUP_HOME/toolchains/nightly-x86_64-unknown-linux-gnu" && \
        rm -f "$RUSTUP_HOME/update-hashes/$RUST_NIGHTLY-x86_64-unknown-linux-gnu" && \
        rustup default nightly; \
    fi && \
    rustc -vV

WORKDIR /workspace
CMD ["/bin/bash"]
