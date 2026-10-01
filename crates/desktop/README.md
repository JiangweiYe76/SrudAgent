# SrudAgent Desktop

Tauri v2 desktop shell with a React + Vite frontend.

## Prerequisites

- Node.js with [pnpm](https://pnpm.io/) 10+
- Rust toolchain (stable)
- A platform webview:
  - **Linux**: `webkit2gtk-4.1`, `gtk3`, `libsoup-3.0`
  - **Windows**: the MSVC toolchain (Visual Studio Build Tools with the
    "Desktop development with C++" workload) and the WebView2 runtime, which
    ships with Windows 11 and most Windows 10 installs

NASM is not required. `aws-lc-sys`, pulled in via `reqwest` → `rustls`, needs
it to assemble its x86_64 Windows code, but `.cargo/config.toml` opts into the
prebuilt objects that crate ships.

## Setup

```bash
pnpm install
```

## Run

```bash
pnpm dev:app
```

This starts the Vite dev server and opens the Tauri window with hot reload.

## Build

```bash
pnpm tauri build
```

Artifacts are written to `target/release/bundle/`.
