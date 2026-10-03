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

// A request the agent refused, with the JSON-RPC code kept apart from the
// message. Some refusals are a client to act on rather than a fault to show —
// asking for a session the agent no longer has is one — and telling them apart
// needs the code, not the sentence.
export class RpcFailure extends Error {
  readonly code: number;

  constructor(code: number, message: string) {
    super(`${code}: ${message}`);
    this.code = code;
    this.name = 'RpcFailure';
  }
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

// The agent's code for a session it does not have. Not a fault: a client asking
// for one is out of date rather than broken, and the two are told apart by what
// the caller should do about it.
export const SESSION_NOT_FOUND = -32001;

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
  if (reply.error) throw new RpcFailure(reply.error.code, reply.error.message);
  return (reply.result ?? {}) as Json;
}

export async function initialize(): Promise<Json> {
  return rpcRequest('initialize', { protocolVersion: 1, clientCapabilities: {} });
}

// Creates a session. `cwd` is where the user wants it to work; an empty string
// means they have not chosen one, and the agent gives the session a workspace of
// its own under its configuration directory. A path that is not an existing
// directory means the same thing to the agent.
//
// ACP has no "not chosen" here — its `session/new` requires an absolute working
// directory and requires the agent to use it:
// https://agentclientprotocol.com/protocol/session-setup#working-directory
// Sending the empty string is SrudAgent's way of saying otherwise.
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

// Hands a client the session's whole conversation as `session/update`
// notifications, arriving before the response. The updates carry the turn each
// belongs to and how each turn ended, since there is no `session/prompt`
// response here to say either.
//
// `cwd` is how ACP has a client confirm the session's directory. It comes from
// `session/list`, which reports the directory of every session it names; an empty
// one means "not chosen", the same word `session/new` accepts, for a client that
// has none of its own.
export async function loadSession(sessionId: string, cwd = ''): Promise<void> {
  await rpcRequest('session/load', { sessionId, cwd, mcpServers: [] });
}

// Renames a session. ACP lets the agent name a session but defines no
// client-to-agent method for renaming one, so this is a SrudAgent extension.
export async function setSessionTitle(sessionId: string, title: string): Promise<void> {
  await rpcRequest('_srud/unstable/session/set_title', { sessionId, title });
}

// Every session the agent knows about, the live ones first and then the stored
// ones newest first. A session outlives the run that created it, so this is how a
// client learns what it could load after a restart.
//
// `updatedAt` is when the session was last active, or null for one the agent is
// holding in memory — a session still in this run is dated by the client, which
// heard of it, rather than by a log that has not been read.
export interface ListedSession {
  sessionId: string;
  cwd: string;
  title: string | null;
  updatedAt: number | null;
}

export async function listSessions(): Promise<ListedSession[]> {
  const result = await rpcRequest('session/list', {});
  const sessions = result.sessions as Json[] | undefined;
  if (!Array.isArray(sessions)) return [];
  return sessions.flatMap((info) => {
    const sessionId = info.sessionId;
    if (typeof sessionId !== 'string') return [];
    const updated = info.updatedAt;
    return [
      {
        sessionId,
        cwd: typeof info.cwd === 'string' ? info.cwd : '',
        title: typeof info.title === 'string' ? info.title : null,
        updatedAt: typeof updated === 'string' ? Date.parse(updated) || null : null,
      },
    ];
  });
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
