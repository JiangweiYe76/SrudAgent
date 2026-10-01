// ACP client over Tauri IPC: JSON-RPC requests go through the `rpc_request`
// command, agent notifications arrive on the `rpc_notify` event. Calls fail
// outside Tauri (plain `vite dev` in a browser), where the app has no backend.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

export type Json = Record<string, unknown>;

interface RpcError {
  code: number;
  message: string;
}

interface RpcReply {
  jsonrpc: string;
  id: number;
  result?: Json;
  error?: RpcError;
}

interface RpcNotifyEnvelope {
  jsonrpc: string;
  method?: string;
  params?: Json;
}

type NotifyHandler = (params: Json) => void;

let seq = 0;
let listening = false;
let handler: NotifyHandler | null = null;

export function isTauri(): boolean {
  return '__TAURI_INTERNALS__' in window;
}

// Registers the callback for agent notifications.
export function onNotify(cb: NotifyHandler): void {
  handler = cb;
}

async function ensureListening(): Promise<void> {
  if (listening) return;
  listening = true;
  await listen<RpcNotifyEnvelope>('rpc_notify', (event) => {
    const msg = event.payload;
    if (!msg.method || !handler) {
      // Diagnostic for the emit→listen bridge: an arriving payload without a
      // method, or arriving before onNotify registered its handler, is
      // silently dropped otherwise.
      console.warn('rpc_notify dropped', JSON.stringify(msg).slice(0, 200));
      return;
    }
    handler((msg.params ?? {}) as Json);
  });
}

export async function rpcRequest(method: string, params: Json): Promise<Json> {
  await ensureListening();
  const id = ++seq;
  const reply = await invoke<RpcReply>('rpc_request', {
    payload: { jsonrpc: '2.0', id, method, params },
  });
  if (reply.error) throw new Error(`${reply.error.code}: ${reply.error.message}`);
  return (reply.result ?? {}) as Json;
}

export async function initialize(): Promise<Json> {
  return rpcRequest('initialize', { protocolVersion: 1, clientCapabilities: {} });
}

export async function newSession(cwd: string): Promise<string> {
  const result = await rpcRequest('session/new', { cwd, mcpServers: [] });
  return String(result.sessionId);
}

// Deletes a session and everything the agent holds for it. Standard ACP, so no
// SrudAgent extension is involved.
export async function deleteSession(sessionId: string): Promise<void> {
  await rpcRequest('session/delete', { sessionId });
}

export async function sendPrompt(sessionId: string, text: string): Promise<string> {
  const result = await rpcRequest('session/prompt', {
    sessionId,
    prompt: [{ type: 'text', text }],
  });
  return String(result.stopReason ?? 'end_turn');
}

// Renames a session. ACP lets the agent name a session but defines no
// client-to-agent method for renaming one, so this is a SrudAgent extension.
export async function setSessionTitle(sessionId: string, title: string): Promise<void> {
  await rpcRequest('_srud/unstable/session/set_title', { sessionId, title });
}

// Asks the agent to interrupt the session's active turn. The in-flight
// `session/prompt` is what resolves, with `stopReason: "cancelled"`; this call
// only signals the turn's cancellation token and returns straight away.
export async function cancelTurn(sessionId: string): Promise<void> {
  await rpcRequest('session/cancel', { sessionId });
}

export function defaultCwd(): Promise<string> {
  return invoke<string>('default_cwd');
}
