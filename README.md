# Coda App

A desktop music player for your local files and a personal [Navidrome](https://www.navidrome.org) server: the GUI sibling of [Coda](https://github.com/JasperSmitz/coda). Built with Tauri 2, a Rust backend on Coda's engine, and a React frontend. Linux and macOS.

**Status:** early development, see [`docs/PLAN.md`](docs/PLAN.md).

## Development

Requirements: Rust (stable), Node 22+, pnpm, [mpv](https://mpv.io) on `PATH`, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) (on Arch: `webkit2gtk-4.1`, `base-devel`, `curl`, `wget`, `file`, `openssl`, `appmenu-gtk-module`, `libappindicator-gtk3`, `librsvg`).

```sh
pnpm install
pnpm tauri dev
```
