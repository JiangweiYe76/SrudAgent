// ACP client over Tauri IPC: JSON-RPC requests go through the `rpc_request`
// command, agent notifications arrive on the `rpc_notify` event. Calls fail
// outside Tauri (plain `vite dev` in a browser), where the app has no backend.
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ModelChoice, ModelGroup, ModelOptions } from './types';

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
export async function newSession(cwd: string): Promise<NewSession> {
  const result = await rpcRequest('session/new', { cwd, mcpServers: [] });
  return {
    sessionId: String(result.sessionId),
    model: parseModelOptions(result.configOptions),
  };
}

// A session the agent created, and the model selector it came with.
export interface NewSession {
  sessionId: string;
  model: ModelOptions | null;
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

// Injects `text` into a turn that is already running. The agent appends it to
// the active turn's context and announces it as a user message; the in-flight
// `session/prompt` stays the single termination signal.
export async function steerTurn(sessionId: string, text: string): Promise<void> {
  await rpcRequest('_srud/unstable/session/steer', {
    sessionId,
    prompt: [{ type: 'text', text }],
  });
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
//
// Returns the model the session was left on, which is what the replayed
// conversation ended with — not necessarily what a fresh session would start on.
export async function loadSession(sessionId: string, cwd = ''): Promise<ModelOptions | null> {
  const result = await rpcRequest('session/load', { sessionId, cwd, mcpServers: [] });
  return parseModelOptions(result.configOptions);
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

// ---------------------------------------------------------------------------
// Choosing the model a session runs on
// ---------------------------------------------------------------------------

// The id of the session config option that selects a model. ACP reserves the
// category `model` for this, and SrudAgent names the option after it.
export const MODEL_OPTION_ID = 'model';

// The group SrudAgent puts a session's current model in when the provider behind
// it is no longer configured. Shown so the selection can be read, and not
// offered as a choice: picking what the session already runs on would change
// nothing.
export const UNAVAILABLE_GROUP = 'unavailable';

function isJson(value: unknown): value is Json {
  return typeof value === 'object' && value !== null;
}

function choicesOf(value: unknown): ModelChoice[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    if (!isJson(entry) || typeof entry.value !== 'string') return [];
    return [
      {
        value: entry.value,
        name: typeof entry.name === 'string' ? entry.name : entry.value,
      },
    ];
  });
}

// The model selector out of a response's `configOptions`, or null when the agent
// offered none — a host with one fixed model client has nothing to choose
// between.
//
// ACP types the set of values as an untagged union of a flat list and a list of
// groups, so the shape is decided by what the entries are rather than by a tag:
// a group is an entry with a `group` key.
export function parseModelOptions(configOptions: unknown): ModelOptions | null {
  if (!Array.isArray(configOptions)) return null;
  const option = configOptions.find(
    (entry) => isJson(entry) && (entry.id === MODEL_OPTION_ID || entry.category === 'model'),
  );
  if (!isJson(option)) return null;

  const raw = Array.isArray(option.options) ? option.options : [];
  const groups: ModelGroup[] = raw.some(
    (entry) => isJson(entry) && typeof entry.group === 'string',
  )
    ? raw.flatMap((entry) => {
        if (!isJson(entry) || typeof entry.group !== 'string') return [];
        return [
          {
            id: entry.group,
            name: typeof entry.name === 'string' ? entry.name : entry.group,
            choices: choicesOf(entry.options),
          },
        ];
      })
    : [{ id: '', name: '', choices: choicesOf(raw) }];

  const currentValue = typeof option.currentValue === 'string' ? option.currentValue : '';
  // A selection nothing offers is kept as its own entry, so the trigger has
  // something to show. The agent does this too, and this only fires for one that
  // did not.
  if (currentValue && !groups.some((g) => g.choices.some((c) => c.value === currentValue))) {
    groups.push({
      id: UNAVAILABLE_GROUP,
      name: '',
      choices: [{ value: currentValue, name: currentValue }],
    });
  }

  return { currentValue, groups };
}

// Switches the model a session runs on. ACP's own method, so a third-party
// client can do this too; only the value's spelling is SrudAgent's.
//
// Returns the whole selector back, rather than an acknowledgement, so the UI
// renders the result of its own change without asking again.
export async function setSessionModel(
  sessionId: string,
  model: string,
): Promise<ModelOptions | null> {
  const result = await rpcRequest('session/set_config_option', {
    sessionId,
    configId: MODEL_OPTION_ID,
    value: model,
  });
  return parseModelOptions(result.configOptions);
}

// ---------------------------------------------------------------------------
// Providers and their credentials
// ---------------------------------------------------------------------------

// Something a model accepts as input, besides the text it always takes.
export type Modality = 'image' | 'audio' | 'video';

// What a model accepts, in a stable order for display.
export const MODALITIES: readonly Modality[] = ['image', 'audio', 'video'];

// One model a provider offers.
//
// `modalities` is what the model accepts besides text; empty is a text-only
// model, and is what every model written before the field existed reads as.
// `contextWindow` is the maximum context in tokens as configured by the user;
// absent means no window was set, and usage is not shown for that model.
export interface ProviderModel {
  id: string;
  name?: string;
  modalities: Modality[];
  contextWindow?: number;
}

// One configured provider, as the panel is shown it.
//
// `apiKey` is a masked rendering — enough to tell two keys apart, not enough to
// use one — and `hasKey` says whether there is one at all. The real credential
// never leaves the agent.
export interface Provider {
  id: string;
  name: string;
  baseUrl: string;
  apiKey?: string;
  hasKey: boolean;
  models: ProviderModel[];
}

// A provider as a write request describes it.
//
// `apiKey` absent means "keep what is stored". That is not the same as clearing
// it: a panel that read a masked key back and saved the provider unchanged must
// not overwrite the real credential with its own display value. The empty string
// is how a key is cleared.
export interface ProviderDraft {
  id: string;
  name: string;
  baseUrl: string;
  apiKey?: string;
  models: ProviderModel[];
}

// Every configured provider, and the model new sessions start from.
export interface ProviderCatalog {
  providers: Provider[];
  default: string | null;
}

// Reads a model's modality list, dropping anything unrecognised. A missing or
// malformed list is a text-only model, which is also what an older record means.
function parseModalities(value: unknown): Modality[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<Modality>();
  for (const entry of value) {
    if (MODALITIES.includes(entry as Modality)) seen.add(entry as Modality);
  }
  return MODALITIES.filter((modality) => seen.has(modality));
}

// A context window only exists as a positive whole number of tokens; anything
// else — absent, malformed, zero — reads as "no window configured".
function parseContextWindow(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isInteger(value) && value > 0 ? value : undefined;
}

function parseCatalog(result: Json): ProviderCatalog {
  const providers = Array.isArray(result.providers) ? result.providers : [];
  return {
    providers: providers.flatMap((entry) => {
      if (!isJson(entry) || typeof entry.id !== 'string') return [];
      const models = Array.isArray(entry.models) ? entry.models : [];
      return [
        {
          id: entry.id,
          name: typeof entry.name === 'string' ? entry.name : entry.id,
          baseUrl: typeof entry.baseUrl === 'string' ? entry.baseUrl : '',
          apiKey: typeof entry.apiKey === 'string' ? entry.apiKey : undefined,
          hasKey: entry.hasKey === true,
          models: models.flatMap((model) =>
            isJson(model) && typeof model.id === 'string'
              ? [
                  {
                    id: model.id,
                    name: typeof model.name === 'string' ? model.name : undefined,
                    modalities: parseModalities(model.modalities),
                    contextWindow: parseContextWindow(model.contextWindow),
                  },
                ]
              : [],
          ),
        },
      ];
    }),
    default: typeof result.default === 'string' ? result.default : null,
  };
}

// The configured providers. Each of the four calls below answers with the whole
// catalog, so the panel renders the result of its own edit without asking again.
export async function listProviders(): Promise<ProviderCatalog> {
  return parseCatalog(await rpcRequest('_srud/unstable/provider/list', {}));
}

// Adds a provider, or replaces the one whose id it names.
export async function saveProvider(provider: ProviderDraft): Promise<ProviderCatalog> {
  return parseCatalog(await rpcRequest('_srud/unstable/provider/set', { provider }));
}

export async function deleteProvider(providerId: string): Promise<ProviderCatalog> {
  return parseCatalog(await rpcRequest('_srud/unstable/provider/delete', { providerId }));
}

// Chooses the model new sessions start from. Sessions already open keep their
// own choice: this is a default, not a broadcast.
export async function setDefaultModel(model: string): Promise<ProviderCatalog> {
  return parseCatalog(await rpcRequest('_srud/unstable/provider/set_default', { model }));
}
