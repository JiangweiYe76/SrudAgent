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

## Troubleshooting

### Blank window on Windows

A window that opens but never paints anything — black or white, no sidebar, no
text — means the WebView2 browser process aborted before it rendered. Chromium
runs a sandbox and code-integrity self-check while starting, and a DLL injected
into the process by a third-party tool makes that check fail: the browser
process `FAIL_FAST`s, the window stays empty, and nothing is written to the
console. Corporate DLP agents and sandboxed launchers are the usual source.

If you hit this, run `pnpm dev:app:no-sandbox`. It passes `--no-sandbox` through
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`, which gets the browser process past the
check. The default script leaves that flag out: it disables the renderer sandbox,
and most machines never need it. Neither script affects `tauri build`.

To find the culprit, compare the DLLs loaded by `desktop.exe` with those of a
process that renders correctly. Anything outside `Windows\` and `Program Files\`
is a candidate.

## Icons

`src-tauri/icons/` holds the generated icon set. Regenerate it from a square
source image with:

```bash
pnpm tauri icon path/to/source.png
```

Windows needs an `.ico` in that set: `tauri-build` embeds it into the
executable, so a missing `icon.ico` fails the build.
