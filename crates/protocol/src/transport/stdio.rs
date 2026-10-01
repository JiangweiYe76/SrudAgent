//! JSON-RPC over stdio — the only transport defined as stable by the ACP
//! specification. SrudAgent's own client talks over Tauri IPC instead; this
//! seam is where a bridge for third-party ACP editors (Zed, Neovim,
//! JetBrains) would attach.
