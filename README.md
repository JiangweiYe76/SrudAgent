> SrudAgent = Srud + Agent.
> Srud comes from Old Chinese 率, meaning “to lead” or “to guide.” Zhengzhang reconstructs it as \*srud; Baxter–Sagart as \*s-rut.

SrudAgent is an AI agent runtime in Rust, with a Tauri desktop shell.

## What it does

SrudAgent implements ACP v1: the server runs the agent, and the desktop acts
as its client.

## Quickstart

### Prerequisites

Rust (stable) and Node.js with pnpm 10+.

- **Linux**: `webkit2gtk-4.1`, `gtk3`, and `libsoup-3.0`.
- **Windows**: the MSVC toolchain (Visual Studio Build Tools) and the
  WebView2 runtime, which ships with Windows 11 and most Windows 10
  installs.

Full platform details are in `crates/desktop/README.md`.

### Run

```sh
pnpm setup
pnpm dev
```

### Build

```sh
pnpm build
```

Artifacts are written to `target/release/bundle/`.
