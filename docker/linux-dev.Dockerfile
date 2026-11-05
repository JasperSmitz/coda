# Linux build/test image for Coda App (Debian trixie, native arch: arm64 on
# Apple Silicon Docker, amd64 elsewhere). Used by scripts/linux-check.sh.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl wget file git pkg-config xdg-utils \
        libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
        librsvg2-dev libssl-dev libxdo-dev patchelf \
        mpv pulseaudio pulseaudio-utils \
        nodejs npm \
        rpm \
    && rm -rf /var/lib/apt/lists/*

# Node 22+: trixie ships Node 20, so take Node 22 from the official tarball.
ARG NODE_VERSION=22.21.1
RUN arch="$(dpkg --print-architecture)"; \
    case "$arch" in amd64) n=x64 ;; arm64) n=arm64 ;; *) echo "unsupported $arch"; exit 1 ;; esac; \
    curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${n}.tar.xz" \
      | tar -xJ -C /usr/local --strip-components=1 \
    && apt-get purge -y nodejs npm && apt-get autoremove -y \
    && npm install -g pnpm@12.8.1 \
    && node --version && pnpm --version

ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal \
        --default-toolchain stable --component rustfmt,clippy \
    && chmod -R a+w /usr/local/rustup /usr/local/cargo \
    && rustc --version

# Linux build output lives here (a named volume), never in the macOS target/.
ENV CARGO_TARGET_DIR=/cargo-target
WORKDIR /work
