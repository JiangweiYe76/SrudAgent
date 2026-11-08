> SrudAgent = Srud + Agent.
> Srud 源自古汉语“率”，意为“引领”或“引导”。郑张尚芳将其构拟为 \*srud；白一平–沙加尔（Baxter–Sagart）构拟为 \*s-rut。

SrudAgent 是一个用 Rust 编写的 AI agent 运行时，桌面端基于 Tauri。

## 简介

SrudAgent 遵循 ACP v1 协议：前端作为 ACP 客户端，通过 JSON-RPC 调用后端的 agent 服务。

## 快速开始

### 环境要求

Rust（stable）、Node.js、pnpm 10+。

- **Linux**：`webkit2gtk-4.1`、`gtk3`、`libsoup-3.0`。
- **Windows**：MSVC 工具链（Visual Studio Build Tools）和 WebView2 运行时
  （Windows 11 和大多数 Windows 10 已预装）。

各平台的详细说明见 `crates/desktop/README.md`。

### 运行

```sh
pnpm setup
pnpm dev
```

### 构建

```sh
pnpm build
```

构建产物在 `target/release/bundle/`。
