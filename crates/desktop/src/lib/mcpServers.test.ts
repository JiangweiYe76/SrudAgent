// @vitest-environment node
//
// The model this app and the agent share: what a server looks like once it is
// written down, what is refused before it is sent, and the two wire shapes it
// has to be spelled in — the agent's catalog is tagged by transport, ACP's
// session request is not.
import { describe, it, expect } from 'vitest';
import {
  asSessionServers,
  emptyServer,
  normalize,
  parsePastedServers,
  readCatalog,
  refusalOf,
  refusalText,
  validateServer,
  type McpHttpSpec,
  type McpServerSpec,
  type McpStdioSpec,
} from './mcpServers';

const stdio = (over: Partial<McpStdioSpec> = {}): McpStdioSpec => ({
  transport: 'stdio',
  name: 'docs',
  command: 'my-mcp-server',
  args: [],
  env: [],
  ...over,
});

const http = (over: Partial<McpHttpSpec> = {}): McpHttpSpec => ({
  transport: 'http',
  name: 'remote',
  url: 'https://example.test/mcp',
  headers: [],
  ...over,
});

describe('a server on its way in', () => {
  it('is trimmed, and its blank rows are dropped', () => {
    const clean = normalize(
      stdio({
        name: '  docs  ',
        command: ' run ',
        args: ['--root', '', '  '],
        env: [
          { name: ' TOKEN ', value: ' t ' },
          { name: '', value: '   ' },
        ],
      }),
    );

    expect(clean).toEqual(
      stdio({
        name: 'docs',
        command: 'run',
        args: ['--root'],
        env: [{ name: 'TOKEN', value: ' t ' }],
      }),
    );
  });

  it('starts empty on either transport', () => {
    expect(emptyServer('stdio')).toEqual({
      transport: 'stdio',
      name: '',
      command: '',
      args: [],
      env: [],
    });
    expect(emptyServer('http')).toEqual({
      transport: 'http',
      name: '',
      url: '',
      headers: [],
    });
  });
});

describe('what is refused before it is sent', () => {
  const problem = (server: McpServerSpec, taken: string[] = []) =>
    validateServer(server, taken)?.key ?? null;

  it('wants a name, and one a tool name can carry', () => {
    expect(problem(stdio({ name: '' }))).toBe('mcp.error.blankName');
    expect(problem(stdio({ name: 'my server' }))).toBe('mcp.error.badName');
    expect(problem(stdio({ name: 'docs/2' }))).toBe('mcp.error.badName');
    expect(problem(stdio({ name: 'docs-2_x' }))).toBeNull();
  });

  it('wants a name no other server has', () => {
    // Two servers whose names differ only in case would still be told apart on
    // the wire, so the comparison does not either.
    expect(problem(stdio({ name: 'docs' }), ['Docs'])).toBe(
      'mcp.error.nameTaken',
    );
    expect(validateServer(stdio({ name: 'docs' }), ['Docs'])?.params).toEqual({
      name: 'docs',
    });
    expect(problem(stdio({ name: 'docs' }), ['other'])).toBeNull();
  });

  it('wants a command, and a URL that is one', () => {
    expect(problem(stdio({ command: '' }))).toBe('mcp.error.blankCommand');
    expect(problem(http({ url: '' }))).toBe('mcp.error.badUrl');
    expect(problem(http({ url: 'example.test/mcp' }))).toBe('mcp.error.badUrl');
    expect(problem(http({ url: 'ftp://example.test' }))).toBe(
      'mcp.error.badUrl',
    );
    expect(problem(http({ url: 'http://127.0.0.1:8080/mcp' }))).toBeNull();
  });

  it('wants every row named, and no row named twice', () => {
    expect(problem(stdio({ env: [{ name: '', value: 'orphan' }] }))).toBe(
      'mcp.error.blankEntryName',
    );
    expect(
      problem(
        http({
          headers: [
            { name: 'Authorization', value: 'a' },
            { name: 'authorization', value: 'b' },
          ],
        }),
      ),
    ).toBe('mcp.error.duplicateEntry');
  });
});

describe('the catalog the agent reports', () => {
  it('reads both transports as this app holds them', () => {
    const servers = readCatalog({
      servers: [
        {
          transport: 'stdio',
          name: 'docs',
          command: 'run',
          args: ['--root'],
          env: [{ name: 'TOKEN', value: 't' }],
        },
        {
          transport: 'http',
          name: 'remote',
          url: 'https://example.test/mcp',
          headers: [],
        },
      ],
    });

    expect(servers).toEqual([
      stdio({
        command: 'run',
        args: ['--root'],
        env: [{ name: 'TOKEN', value: 't' }],
      }),
      http(),
    ]);
  });

  it('costs the record it cannot read, not the list', () => {
    const servers = readCatalog({
      servers: [
        { transport: 'stdio', name: 'ok', command: 'run' },
        { transport: 'stdio', name: 'no-command' },
        { transport: 'carrier-pigeon', name: 'odd', command: 'run' },
        'not a server at all',
      ],
    });

    expect(servers.map((server) => server.name)).toEqual(['ok', 'odd']);
  });

  it('reports nothing when the reply holds no list', () => {
    expect(readCatalog({})).toEqual([]);
    expect(readCatalog({ servers: 'docs' })).toEqual([]);
  });
});

describe('what a session is sent', () => {
  it('sends stdio without a transport tag', () => {
    // ACP's `McpServer` is tagged by `type` with the stdio variant untagged: a
    // `type` field here would name a transport the agent does not expect.
    const [wire] = asSessionServers([stdio({ args: ['--root'] })]);

    expect(wire).toEqual({
      name: 'docs',
      command: 'my-mcp-server',
      args: ['--root'],
      env: [],
    });
    expect(wire).not.toHaveProperty('type');
  });

  it('sends HTTP tagged, with its headers', () => {
    const [wire] = asSessionServers([
      http({ headers: [{ name: 'Authorization', value: 'Bearer t' }] }),
    ]);

    expect(wire).toEqual({
      type: 'http',
      name: 'remote',
      url: 'https://example.test/mcp',
      headers: [{ name: 'Authorization', value: 'Bearer t' }],
    });
  });

  it('sends nothing when nothing is configured', () => {
    expect(asSessionServers([])).toEqual([]);
  });
});

describe('a refusal the agent sent', () => {
  it('is read as the rule it names, with the values it places', () => {
    const refusal = refusalOf({
      detail: '`https://` is not an HTTP or HTTPS URL',
      data: { kind: 'bad-url', params: { url: 'https://' } },
    });

    expect(refusal).toEqual({
      detail: '`https://` is not an HTTP or HTTPS URL',
      kind: 'bad-url',
      params: { url: 'https://' },
    });
    expect(refusalText(refusal)).toBe(
      'The URL must be an absolute HTTP or HTTPS URL.',
    );
  });

  it('is shown as the agent wrote it when the rule has no wording here', () => {
    const refusal = refusalOf({
      detail: 'something this build has not heard of',
      data: { kind: 'a-newer-rule', params: {} },
    });

    expect(refusalText(refusal)).toBe('something this build has not heard of');
  });

  it('is read as a plain failure when there was no reply at all', () => {
    const refusal = refusalOf(new Error('the bridge is gone'));

    expect(refusal.kind).toBeNull();
    expect(refusalText(refusal)).toContain('the bridge is gone');
  });
});

// What someone pastes is copied from another client's documentation, so the
// reader accepts the shapes those documents print. It is not lenient about the
// outcome: one entry it cannot read means the paste is not imported at all.
describe('a pasted configuration', () => {
  it('is read from the wrapper every other client writes', () => {
    const parsed = parsePastedServers(`{
      "mcpServers": {
        "context7": {
          "command": "npx",
          "args": ["-y", "@upstash/context7-mcp"]
        }
      }
    }`);

    expect(parsed.problems).toEqual([]);
    expect(parsed.servers).toEqual([
      stdio({
        name: 'context7',
        command: 'npx',
        args: ['-y', '@upstash/context7-mcp'],
      }),
    ]);
  });

  it('is read from a name-keyed object with no wrapper', () => {
    const parsed = parsePastedServers(
      '{"docs": {"command": "run"}, "remote": {"url": "https://example.test/mcp"}}',
    );

    expect(parsed.problems).toEqual([]);
    expect(parsed.servers).toEqual([
      stdio({ name: 'docs', command: 'run' }),
      http({ name: 'remote' }),
    ]);
  });

  it('takes a list of named servers, which is how this app writes one out', () => {
    const parsed = parsePastedServers(
      JSON.stringify({
        servers: [
          { name: 'docs', command: 'run', args: [], env: {} },
          { name: 'remote', url: 'https://example.test/mcp' },
        ],
      }),
    );

    expect(parsed.problems).toEqual([]);
    expect(parsed.servers).toEqual([
      stdio({ name: 'docs', command: 'run' }),
      http({ name: 'remote' }),
    ]);
  });

  it('reads variables and headers written as an object', () => {
    const parsed = parsePastedServers(`{
      "mcpServers": {
        "docs": { "command": "run", "env": { "TOKEN": "t" } },
        "remote": {
          "url": "https://example.test/mcp",
          "headers": { "Authorization": "Bearer t" }
        }
      }
    }`);

    expect(parsed.servers).toEqual([
      stdio({
        name: 'docs',
        command: 'run',
        env: [{ name: 'TOKEN', value: 't' }],
      }),
      http({
        name: 'remote',
        headers: [{ name: 'Authorization', value: 'Bearer t' }],
      }),
    ]);
  });

  it('ignores the keys another client adds', () => {
    const parsed = parsePastedServers(
      '{"context7": {"command": "npx", "args": [], "disabled": false, "autoApprove": ["search"]}}',
    );

    expect(parsed.problems).toEqual([]);
    expect(parsed.servers).toEqual([
      stdio({ name: 'context7', command: 'npx' }),
    ]);
  });

  it('refuses a transport it does not speak, rather than reading it as HTTP', () => {
    const parsed = parsePastedServers(
      '{"legacy": {"type": "sse", "url": "https://example.test/sse"}}',
    );

    expect(parsed.servers).toEqual([]);
    expect(parsed.problems).toEqual([
      {
        key: 'mcp.paste.unsupportedTransport',
        params: { name: 'legacy', type: 'sse' },
      },
    ]);
  });

  it('says what is wrong with the text before anything else', () => {
    expect(parsePastedServers('   ').problems).toEqual([
      { key: 'mcp.paste.empty' },
    ]);
    expect(parsePastedServers('{"mcpServers": ').problems).toEqual([
      { key: 'mcp.paste.badJson' },
    ]);
    expect(parsePastedServers('{"version": 1}').problems).toEqual([
      { key: 'mcp.paste.unreadable' },
    ]);
    expect(parsePastedServers('{"mcpServers": {}}').problems).toEqual([
      { key: 'mcp.paste.noServers' },
    ]);
  });

  it('names the entry it could not read, by its position when it has no name', () => {
    const parsed = parsePastedServers('[{"command": "run"}]');

    expect(parsed.problems).toEqual([
      { key: 'mcp.paste.missingName', params: { index: '1' } },
    ]);
  });

  it('refuses arguments that are not strings', () => {
    const parsed = parsePastedServers(
      '{"docs": {"command": "run", "args": [1, 2]}}',
    );

    expect(parsed.problems).toEqual([
      { key: 'mcp.paste.badArgs', params: { name: 'docs' } },
    ]);
  });

  it('refuses a name that is not there, twice', () => {
    const parsed = parsePastedServers(
      '{"mcpServers": {"docs": {"command": "a"}, "Docs": {"command": "b"}}}',
    );

    expect(parsed.problems).toEqual([
      { key: 'mcp.error.nameTaken', params: { name: 'Docs' } },
    ]);
  });

  it('refuses a server with nothing to start or dial', () => {
    const parsed = parsePastedServers('{"docs": {"args": []}}');

    expect(parsed.problems).toEqual([
      { key: 'mcp.error.noTransport', params: { name: 'docs' } },
    ]);
  });
});
