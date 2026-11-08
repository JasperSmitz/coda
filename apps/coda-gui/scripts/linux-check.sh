#!/usr/bin/env bash
# Builds and checks Coda App on Linux inside Docker (see docker/linux-dev.Dockerfile).
#
#   scripts/linux-check.sh            # fmt, clippy, tests (+ ignored mpv tests), pnpm build, tauri build --debug --no-bundle
#   scripts/linux-check.sh --bundle   # additionally a release deb + AppImage (+ rpm)
#   scripts/linux-check.sh --shell    # an interactive shell in the container
#
# The whole monorepo is mounted read-write at /work (the Cargo workspace is
# at its root) and the commands run in /work/apps/coda-gui. Cargo's registry,
# the Linux target dir and node_modules are named Docker volumes, so macOS and
# Linux build output (and native node modules) never mix.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
repo="$(cd "$root/../.." && pwd)"
image="coda-app-linux-dev"
mode="${1:-check}"

docker build -t "$image" -f "$root/docker/linux-dev.Dockerfile" "$root/docker"

run=(docker run --rm
  -v "$repo:/work"
  -v coda-app-cargo-registry:/usr/local/cargo/registry
  -v coda-app-cargo-git:/usr/local/cargo/git
  -v coda-app-linux-target:/cargo-target
  -v coda-app-node-modules:/work/apps/coda-gui/node_modules
  -v coda-app-pnpm-store:/root/.local/share/pnpm/store
  -e CI=true
  -w /work/apps/coda-gui)

if [ "$mode" = "--shell" ]; then
  exec "${run[@]}" -it "$image" bash
fi

bundle=0
[ "$mode" = "--bundle" ] && bundle=1

# The script below runs in the container, so it is single-quoted on purpose.
# shellcheck disable=SC2016
"${run[@]}" -e BUNDLE="$bundle" "$image" bash -euo pipefail -c '
  step() { printf "\n==> %s\n" "$*"; }
  step "versions"; rustc --version; node --version; pnpm --version; mpv --version | head -1
  # The Rust build embeds dist/, so the frontend is built first.
  step "pnpm install"; pnpm install --frozen-lockfile
  step "pnpm build";   pnpm build
  step "cargo fmt";    cargo fmt --all -- --check
  step "cargo clippy"; cargo clippy --locked -p coda-app -p coda-core --all-targets -- -D warnings
  step "cargo test";   cargo test --locked -p coda-app -p coda-core
  # Some mpv tests use the default audio output of mpv; with no sound card,
  # PulseAudio falls back to a null sink (as in the Coda CI).
  step "pulseaudio"; pulseaudio --start --exit-idle-time=-1 && pactl info | grep -E "Server Name|Default Sink"
  step "cargo test (ignored, needs mpv)"; cargo test --locked -p coda-app -p coda-core -- --ignored
  step "tauri build --debug --no-bundle"; pnpm tauri build --debug --no-bundle
  file "$CARGO_TARGET_DIR/debug/coda-app"
  if [ "$BUNDLE" = 1 ]; then
    step "tauri build (release bundles)"; pnpm tauri build --bundles deb,appimage,rpm -- --profile release-gui
    ls -l "$CARGO_TARGET_DIR"/release-gui/bundle/*/
    for d in "$CARGO_TARGET_DIR"/release-gui/bundle/deb/*.deb; do dpkg-deb --info "$d"; dpkg-deb --contents "$d"; done
  fi
  step "all Linux checks passed"
'
