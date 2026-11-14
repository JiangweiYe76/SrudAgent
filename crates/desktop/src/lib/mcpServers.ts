// The MCP servers this app's sessions run with.
//
// ACP carries the list per session — `session/new` and `session/load` both take
// `mcpServers`, and the agent starts what it is given — so the list is the
// client's to hold. The agent keeps it between sessions in `mcp.json` under its
// configuration directory, a file the user may also edit by hand, and this
// module is the shape both sides agree on.
//
// Two shapes meet here and they are not the same: what the agent's catalog
// reports is tagged by transport, while what ACP's session request takes is not.
import { t, type Key } from './i18n';

/** One environment variable, or one HTTP header. */
export interface McpEntry {
  name: string;
  value: string;
}

/** A server the agent starts as a child process. */
export interface McpStdioSpec {
  transport: 'stdio';
  name: string;
  command: string;
  args: string[];
  env: McpEntry[];
}

/** A server the agent reaches over Streamable HTTP. */
export interface McpHttpSpec {
  transport: 'http';
  name: string;
  url: string;
  headers: McpEntry[];
}

export type McpServerSpec = McpStdioSpec | McpHttpSpec;

export type McpTransport = McpServerSpec['transport'];

// The characters a name may carry. The agent builds every tool name as
// `mcp__<server>__<tool>` and replaces anything else with `_`, so a name that
// needed replacing could collide with another server's.
const NAME_PATTERN = /^[A-Za-z0-9_-]+$/;

/** A new server of the given transport, with nothing filled in. */
export function emptyServer(transport: McpTransport): McpServerSpec {
  return transport === 'http'
    ? { transport, name: '', url: '', headers: [] }
    : { transport, name: '', command: '', args: [], env: [] };
}

// What a server looks like once it is written down: trimmed, and without the
// rows someone added and then left empty.
export function normalize(server: McpServerSpec): McpServerSpec {
  const name = server.name.trim();
  const entries = (list: readonly McpEntry[]): McpEntry[] =>
    list
      .map((entry) => ({ name: entry.name.trim(), value: entry.value }))
      .filter((entry) => entry.name !== '' || entry.value.trim() !== '');
  if (server.transport === 'http')
    return {
      transport: 'http',
      name,
      url: server.url.trim(),
      headers: entries(server.headers),
    };
  return {
    transport: 'stdio',
    name,
    command: server.command.trim(),
    // Every argument is its own row, so an empty one is a row left blank
    // rather than an empty argument being passed.
    args: server.args.filter((arg) => arg.trim() !== ''),
    env: entries(server.env),
  };
}

/** The wording for a refused server, if it is refused. */
export interface McpProblem {
  key: Key;
  params?: Record<string, string>;
}

/** What is wrong with this server, or `null` when nothing is. */
export function validateServer(
  server: McpServerSpec,
  taken: readonly string[],
): McpProblem | null {
  const name = server.name;
  if (name === '') return { key: 'mcp.error.blankName' };
  if (!NAME_PATTERN.test(name)) return { key: 'mcp.error.badName' };
  if (taken.some((other) => other.toLowerCase() === name.toLowerCase()))
    return { key: 'mcp.error.nameTaken', params: { name } };
  if (server.transport === 'http') {
    if (!isHttpUrl(server.url)) return { key: 'mcp.error.badUrl' };
    return entryProblem(server.headers);
  }
  if (server.command === '') return { key: 'mcp.error.blankCommand' };
  return entryProblem(server.env);
}

function isHttpUrl(url: string): boolean {
  return /^https?:\/\/\S+$/.test(url);
}

// A row named twice would have one of its two values silently win, and an
// unnamed row cannot be sent at all.
function entryProblem(entries: readonly McpEntry[]): McpProblem | null {
  const seen = new Set<string>();
  for (const entry of entries) {
    if (entry.name === '') return { key: 'mcp.error.blankEntryName' };
    const name = entry.name.toLowerCase();
    if (seen.has(name))
      return { key: 'mcp.error.duplicateEntry', params: { name: entry.name } };
    seen.add(name);
  }
  return null;
}

/** What is wrong with something pasted, in the wording this build has for it. */
export interface PasteProblem {
  key: Key;
  params?: Record<string, string>;
}

/** The servers a paste described, and everything that stopped the rest. */
export interface PasteResult {
  servers: McpServerSpec[];
  problems: PasteProblem[];
}

// The wrappers a pasted configuration arrives in. `mcpServers` is the one every
// other client writes; `servers` is what this app's own catalog and file use, so
// a configuration copied out of either can be pasted back in.
const ENTRY_KEYS = ['mcpServers', 'servers'];

/**
 * Reads the text of a pasted configuration.
 *
 * Foreign formats are read leniently — a name-keyed object, an array of named
 * servers, `type` tags this build does not need — because what someone pastes is
 * copied from another client's documentation rather than written for this one.
 * What is not lenient is the outcome: every problem is collected, and a paste
 * with one is not imported at all, so a field this build could not read never
 * turns into a server that runs without it.
 */
export function parsePastedServers(text: string): PasteResult {
  const body = text.trim();
  if (body === '') return refused('mcp.paste.empty');

  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return refused('mcp.paste.badJson');
  }

  const entries = pastedEntries(parsed);
  if (entries === null) return refused('mcp.paste.unreadable');
  if (entries.length === 0) return refused('mcp.paste.noServers');

  const servers: McpServerSpec[] = [];
  const problems: PasteProblem[] = [];
  entries.forEach(({ name, config }, index) => {
    const read = pastedServer(name, config, index);
    if ('problem' in read) {
      problems.push(read.problem);
      return;
    }
    const duplicate = servers.some(
      (server) => server.name.toLowerCase() === read.server.name.toLowerCase(),
    );
    if (duplicate) {
      problems.push({
        key: 'mcp.error.nameTaken',
        params: { name: read.server.name },
      });
      return;
    }
    servers.push(read.server);
  });

  return problems.length > 0
    ? { servers: [], problems }
    : { servers, problems };
}

function refused(key: Key): PasteResult {
  return { servers: [], problems: [{ key }] };
}

/** One server of a paste, named either by its key or by its own `name`. */
function pastedEntries(
  value: unknown,
): Array<{ name: string | null; config: unknown }> | null {
  if (Array.isArray(value)) {
    return value.map((entry) => {
      const record =
        typeof entry === 'object' && entry !== null && !Array.isArray(entry)
          ? (entry as Record<string, unknown>)
          : {};
      return {
        name: typeof record.name === 'string' ? record.name : null,
        config: entry,
      };
    });
  }
  if (typeof value !== 'object' || value === null) return null;

  const record = value as Record<string, unknown>;
  for (const key of ENTRY_KEYS) {
    const inner = record[key];
    if (inner === undefined) continue;
    const entries = pastedEntries(inner);
    if (entries !== null) return entries;
  }
  // A bare name-keyed object: `{ "context7": { "command": … } }`.
  const keys = Object.keys(record);
  if (keys.length === 0) return [];
  const allObjects = keys.every((key) => {
    const entry = record[key];
    return typeof entry === 'object' && entry !== null && !Array.isArray(entry);
  });
  return allObjects
    ? keys.map((name) => ({ name, config: record[name] }))
    : null;
}

function pastedServer(
  rawName: string | null,
  config: unknown,
  index: number,
): { server: McpServerSpec } | { problem: PasteProblem } {
  if (rawName === null)
    return {
      problem: {
        key: 'mcp.paste.missingName',
        params: { index: String(index + 1) },
      },
    };
  const name = rawName.trim();
  if (!NAME_PATTERN.test(name))
    return { problem: { key: 'mcp.error.badName' } };
  if (typeof config !== 'object' || config === null || Array.isArray(config))
    return { problem: { key: 'mcp.paste.notAnObject', params: { name } } };

  const record = config as Record<string, unknown>;
  const tag = [record.type, record.transport].find(
    (value): value is string => typeof value === 'string',
  );
  // The one transport this build does not speak. Saying so is the point: an
  // `sse` entry read as HTTP would dial the wrong protocol and fail later, at
  // the first request, where nothing connects it to the paste.
  if (tag?.toLowerCase() === 'sse')
    return {
      problem: {
        key: 'mcp.paste.unsupportedTransport',
        params: { name, type: tag },
      },
    };

  const url = typeof record.url === 'string' ? record.url : '';
  if (url !== '') {
    const headers = pastedEntriesOf(record.headers);
    if (headers === null)
      return { problem: { key: 'mcp.paste.badEntries', params: { name } } };
    return {
      server: normalize({
        transport: 'http',
        name,
        url,
        headers,
      }),
    };
  }

  const command = typeof record.command === 'string' ? record.command : '';
  if (command !== '') {
    const args = pastedArgs(record.args);
    if (args === null)
      return { problem: { key: 'mcp.paste.badArgs', params: { name } } };
    const env = pastedEntriesOf(record.env);
    if (env === null)
      return { problem: { key: 'mcp.paste.badEntries', params: { name } } };
    return {
      server: normalize({ transport: 'stdio', name, command, args, env }),
    };
  }

  return { problem: { key: 'mcp.error.noTransport', params: { name } } };
}

// Variables and headers arrive either as an object of strings, which is what a
// configuration file holds, or as rows, which is what this app's catalog
// reports. Both round-trip; anything else is a field nobody can act on.
function pastedEntriesOf(value: unknown): McpEntry[] | null {
  if (value === undefined || value === null) return [];
  if (Array.isArray(value)) {
    const rows: McpEntry[] = [];
    for (const row of value) {
      if (typeof row !== 'object' || row === null) return null;
      const record = row as Record<string, unknown>;
      if (typeof record.name !== 'string' || typeof record.value !== 'string')
        return null;
      rows.push({ name: record.name, value: record.value });
    }
    return rows;
  }
  if (typeof value !== 'object') return null;
  const rows: McpEntry[] = [];
  for (const [name, entry] of Object.entries(
    value as Record<string, unknown>,
  )) {
    if (typeof entry !== 'string') return null;
    rows.push({ name, value: entry });
  }
  return rows;
}

function pastedArgs(value: unknown): string[] | null {
  if (value === undefined || value === null) return [];
  if (!Array.isArray(value)) return null;
  const args: string[] = [];
  for (const arg of value) {
    if (typeof arg !== 'string') return null;
    args.push(arg);
  }
  return args;
}

/** One tool a server declares, as the agent lists it. */
export interface McpTool {
  /** The name the server knows it by, before the `mcp__<server>__` prefix. */
  name: string;
  /** What the server says it does, empty when it says nothing. */
  description: string;
}

/** The tools the agent reported for one server. */
export function readTools(result: Record<string, unknown>): McpTool[] {
  const tools = result.tools;
  if (!Array.isArray(tools)) return [];
  return tools.flatMap((entry) => {
    if (typeof entry !== 'object' || entry === null) return [];
    const record = entry as Record<string, unknown>;
    if (typeof record.name !== 'string') return [];
    return [
      {
        name: record.name,
        description:
          typeof record.description === 'string' ? record.description : '',
      },
    ];
  });
}

/** A refusal the agent sent, in the shape its wording needs. */
export interface McpRefusal {
  /** The agent's own words, shown when the rule has no wording here. */
  detail: string;
  /** The rule the agent named, when it names one. */
  kind: string | null;
  /** The values that rule's wording places. */
  params: Record<string, string>;
}

// The wording each rule has here. One this table has never heard of is printed
// as the agent wrote it, so a client written before a rule still says something
// true rather than nothing.
const REFUSAL_KEYS: Record<string, Key> = {
  'blank-name': 'mcp.error.blankName',
  'bad-name': 'mcp.error.badName',
  'no-transport': 'mcp.error.noTransport',
  'two-transports': 'mcp.error.twoTransports',
  'blank-command': 'mcp.error.blankCommand',
  'bad-url': 'mcp.error.badUrl',
  'blank-entry-name': 'mcp.error.blankEntryName',
  'duplicate-entry': 'mcp.error.duplicateEntry',
  'duplicate-name': 'mcp.error.nameTaken',
  'unknown-server': 'mcp.error.unknownServer',
};

/** What to show for a refused change, in this build's words. */
export function refusalText(refusal: McpRefusal): string {
  const key = refusal.kind === null ? undefined : REFUSAL_KEYS[refusal.kind];
  return key === undefined ? refusal.detail : t(key, refusal.params);
}

/**
 * The refusal behind an error from the RPC layer, if it carries one.
 *
 * Read by shape rather than by `instanceof`: the RPC module imports this one for
 * the session list, so naming its class here would close a cycle.
 */
export function refusalOf(err: unknown): McpRefusal {
  const reply = err as { detail?: unknown; data?: unknown } | null;
  if (typeof reply?.detail !== 'string') {
    // A transport failure: no reply, so nothing to word ourselves.
    return { detail: String(err), kind: null, params: {} };
  }
  const data = (reply.data ?? {}) as { kind?: unknown; params?: unknown };
  return {
    detail: reply.detail,
    kind: typeof data.kind === 'string' ? data.kind : null,
    params: isParams(data.params) ? data.params : {},
  };
}

function isParams(value: unknown): value is Record<string, string> {
  return (
    typeof value === 'object' &&
    value !== null &&
    Object.values(value).every((entry) => typeof entry === 'string')
  );
}

/** The servers the agent reports, as this app holds them. */
export function readCatalog(result: Record<string, unknown>): McpServerSpec[] {
  const servers = result.servers;
  if (!Array.isArray(servers)) return [];
  return servers.flatMap((entry) => {
    const server = readServer(entry);
    return server === null ? [] : [server];
  });
}

// One catalog entry, or `null` when it is not a server this build understands.
// A record that does not parse costs one row, not the list.
function readServer(value: unknown): McpServerSpec | null {
  if (typeof value !== 'object' || value === null) return null;
  const record = value as Record<string, unknown>;
  if (typeof record.name !== 'string') return null;
  const name = record.name;
  if (record.transport === 'http') {
    if (typeof record.url !== 'string') return null;
    return {
      transport: 'http',
      name,
      url: record.url,
      headers: readEntries(record.headers),
    };
  }
  if (typeof record.command !== 'string') return null;
  return {
    transport: 'stdio',
    name,
    command: record.command,
    args: Array.isArray(record.args)
      ? record.args.filter((arg): arg is string => typeof arg === 'string')
      : [],
    env: readEntries(record.env),
  };
}

function readEntries(value: unknown): McpEntry[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((entry) => {
    if (typeof entry !== 'object' || entry === null) return [];
    const record = entry as Record<string, unknown>;
    if (typeof record.name !== 'string' || typeof record.value !== 'string')
      return [];
    return [{ name: record.name, value: record.value }];
  });
}

/** The type `McpToolInfo` is an alias of `McpTool`. */
export type McpToolInfo = McpTool;

/** Every configured server, in the order the file lists them. */
export interface McpCatalog {
  servers: McpServerSpec[];
}

/** The `mcpServers` array a session request carries. */
export type SessionMcpServer = Record<string, unknown>;

export function asSessionServers(
  servers: readonly McpServerSpec[],
): SessionMcpServer[] {
  return servers.map((server) => {
    if (server.transport === 'http')
      return {
        type: 'http',
        name: server.name,
        url: server.url,
        headers: server.headers.map((header) => ({
          name: header.name,
          value: header.value,
        })),
      };
    // stdio carries no tag. ACP's `McpServer` is tagged by `type` with the
    // stdio variant untagged, which is what makes a bare `{ name, command }`
    // read as the transport every agent must support.
    return {
      name: server.name,
      command: server.command,
      args: server.args,
      env: server.env.map((variable) => ({
        name: variable.name,
        value: variable.value,
      })),
    };
  });
}
