# SrudAgent Desktop

Tauri v2 desktop shell with a React + Vite frontend.

## Prerequisites

- Node.js with [pnpm](https://pnpm.io/) 10+
- Rust toolchain (stable)
- Linux system libraries: `webkit2gtk-4.1`, `gtk3`, `libsoup-3.0`

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
