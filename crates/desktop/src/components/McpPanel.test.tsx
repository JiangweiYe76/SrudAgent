// @vitest-environment jsdom
//
// The panel is a skin over the agent's own methods: it asks for the list, sends
// one server at a time, and renders whatever comes back. So the tests are about
// what leaves for the agent — the tagged catalog shape, one server per save —
// and what the pane does with a refusal.
import {
  describe,
  it,
  expect,
  vi,
  beforeAll,
  beforeEach,
  afterEach,
} from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import type { McpServerSpec } from '@/lib/mcpServers';

const mocks = vi.hoisted(() => ({
  invoke: async (
    _cmd: string,
    _args?: Record<string, unknown>,
  ): Promise<unknown> => {
    throw new Error('invoke not configured');
  },
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) =>
    mocks.invoke(cmd, args),
}));

vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

const stdio = (
  over: Partial<Extract<McpServerSpec, { transport: 'stdio' }>> = {},
) =>
  ({
    transport: 'stdio',
    name: 'docs',
    command: 'my-mcp-server',
    args: [],
    env: [],
    ...over,
  }) as McpServerSpec;

const http = (
  over: Partial<Extract<McpServerSpec, { transport: 'http' }>> = {},
) =>
  ({
    transport: 'http',
    name: 'remote',
    url: 'https://example.test/mcp',
    headers: [],
    ...over,
  }) as McpServerSpec;

const reply = (id: number, result: unknown) => ({ jsonrpc: '2.0', id, result });
const refuse = (
  id: number,
  error: { code: number; message: string; data?: unknown },
) => ({ jsonrpc: '2.0', id, error });

let catalog: McpServerSpec[];
// What the fake server declares, and whether asking it fails instead.
let tools: Array<{ name: string; description: string }>;
let toolsFailure: string | null;
let sent: Array<{ method: string; params: Record<string, unknown> }>;
// Answers the next request of the named method with a refusal instead.
let refuseNext: {
  method: string;
  error: { code: number; message: string; data?: unknown };
} | null;

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(async () => {
  vi.resetModules();
  catalog = [];
  tools = [];
  toolsFailure = null;
  sent = [];
  refuseNext = null;
  mocks.invoke = async (cmd, args) => {
    if (cmd !== 'rpc_request') throw new Error(`unexpected invoke: ${cmd}`);
    const payload = args!.payload as {
      id: number;
      method: string;
      params: Record<string, unknown>;
    };
    sent.push({ method: payload.method, params: payload.params });
    if (refuseNext?.method === payload.method) {
      const error = refuseNext.error;
      refuseNext = null;
      return refuse(payload.id, error);
    }
    switch (payload.method) {
      case '_srud/unstable/mcp/list':
        return reply(payload.id, { servers: catalog });
      case '_srud/unstable/mcp/set': {
        const server = payload.params.server as McpServerSpec;
        const index = catalog.findIndex((entry) => entry.name === server.name);
        catalog =
          index === -1
            ? [...catalog, server]
            : catalog.map((entry, i) => (i === index ? server : entry));
        return reply(payload.id, { servers: catalog });
      }
      case '_srud/unstable/mcp/delete':
        catalog = catalog.filter((entry) => entry.name !== payload.params.name);
        return reply(payload.id, { servers: catalog });
      case '_srud/unstable/mcp/tools':
        if (toolsFailure !== null) {
          return refuse(payload.id, { code: -32603, message: toolsFailure });
        }
        return reply(payload.id, { tools });
      case '_srud/unstable/mcp/import': {
        const incoming = payload.params.servers as McpServerSpec[];
        const added: string[] = [];
        const replaced: string[] = [];
        for (const server of incoming) {
          const index = catalog.findIndex(
            (entry) => entry.name.toLowerCase() === server.name.toLowerCase(),
          );
          if (index === -1) {
            catalog = [...catalog, server];
            added.push(server.name);
          } else {
            replaced.push(catalog[index].name);
            catalog = catalog.map((entry, i) => (i === index ? server : entry));
          }
        }
        return reply(payload.id, { servers: catalog, added, replaced });
      }
      default:
        throw new Error(`unexpected method: ${payload.method}`);
    }
  };
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function render(): Promise<void> {
  const { McpPanel } = await import('./McpPanel');
  await act(async () => {
    root.render(<McpPanel />);
  });
  await vi.waitFor(() => {
    expect(
      sent.some((entry) => entry.method === '_srud/unstable/mcp/list'),
    ).toBe(true);
  });
}

function button(label: string): HTMLButtonElement {
  const found = [
    ...document.querySelectorAll<HTMLButtonElement>('button'),
  ].find(
    (b) =>
      b.getAttribute('aria-label') === label || b.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no button labelled ${label}`);
  return found;
}

function buttons(label: string): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>('button')].filter(
    (b) =>
      b.getAttribute('aria-label') === label || b.textContent?.trim() === label,
  );
}

// The row of a server in the list, which is a button around the name and the
// summary beneath it.
function row(name: string): HTMLButtonElement {
  const found = [
    ...document.querySelectorAll<HTMLButtonElement>('li button'),
  ].find((b) => b.querySelector('span')?.textContent === name);
  if (!found) throw new Error(`no row for ${name}`);
  return found;
}

async function click(element: HTMLElement): Promise<void> {
  await act(async () => {
    element.click();
  });
}

// The input of the field labelled `label`. A field is a `<label>` wrapping its
// own caption, which is how the form builds every named control.
function inputByLabel(label: string): HTMLInputElement {
  const found = [...document.querySelectorAll('label')]
    .find((l) => l.querySelector('span')?.textContent === label)
    ?.querySelector('input');
  if (!found) throw new Error(`no field labelled ${label}`);
  return found;
}

function inputByPlaceholder(placeholder: string): HTMLInputElement {
  const found = document.querySelector<HTMLInputElement>(
    `input[placeholder="${placeholder}"]`,
  );
  if (!found) throw new Error(`no input with placeholder ${placeholder}`);
  return found;
}

// Types into a controlled input the way a browser does: through the native
// value setter, so React's own value tracker sees the change.
async function type(element: HTMLInputElement, value: string): Promise<void> {
  const set = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    'value',
  )!.set!;
  await act(async () => {
    set.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

// The same, for the one control that holds a pasted configuration.
async function paste(text: string): Promise<void> {
  const area = document.querySelector('textarea');
  if (!area) throw new Error('no textarea');
  const set = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    'value',
  )!.set!;
  await act(async () => {
    set.call(area, text);
    area.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

/** The last `set` the panel sent, as the agent received it. */
function lastSet(): McpServerSpec {
  const found = [...sent]
    .reverse()
    .find((entry) => entry.method === '_srud/unstable/mcp/set');
  if (!found) throw new Error('no set was sent');
  return found.params.server as McpServerSpec;
}

describe('adding a server', () => {
  it('sends it as an argument of its own row', async () => {
    await render();

    await click(button('Add server'));
    await type(inputByLabel('Name'), 'docs');
    await type(inputByLabel('Command'), 'my-mcp-server');
    await click(button('Add argument'));
    await type(inputByPlaceholder('--flag'), '--root');
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(lastSet()).toEqual(stdio({ args: ['--root'] }));
    });
    expect(document.body.textContent).toContain('my-mcp-server --root');
  });

  it('sends an environment variable as its own row', async () => {
    await render();

    await click(button('Add server'));
    await type(inputByLabel('Name'), 'docs');
    await type(inputByLabel('Command'), 'run');
    await click(button('Add variable'));
    await type(inputByPlaceholder('NAME'), 'TOKEN');
    await type(inputByPlaceholder('value'), 'secret');
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(lastSet()).toEqual(
        stdio({ command: 'run', env: [{ name: 'TOKEN', value: 'secret' }] }),
      );
    });
  });

  it('sends an HTTP server with its URL', async () => {
    await render();

    await click(button('Add server'));
    await click(button('HTTP'));
    await type(inputByLabel('Name'), 'remote');
    await type(inputByLabel('URL'), 'https://example.test/mcp');
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(lastSet()).toEqual(http());
    });
  });
});

describe('a name that is already taken', () => {
  it('is refused here, without asking the agent', async () => {
    catalog = [stdio()];
    await render();

    await click(button('Add server'));
    await type(inputByLabel('Name'), 'Docs');
    await type(inputByLabel('Command'), 'other');

    expect(document.body.textContent).toContain(
      'Another server is already called',
    );
    expect(button('Save').disabled).toBe(true);
    expect(
      sent.filter((entry) => entry.method === '_srud/unstable/mcp/set'),
    ).toEqual([]);
  });
});

describe('the list', () => {
  it('is what the agent reports, with the command it runs', async () => {
    catalog = [stdio({ args: ['--root'] }), http()];
    await render();

    expect(document.body.textContent).toContain('docs');
    expect(document.body.textContent).toContain('my-mcp-server --root');
    expect(document.body.textContent).toContain('https://example.test/mcp');
  });

  it('drops the row a delete names, by that name', async () => {
    catalog = [stdio(), http()];
    await render();

    await click(buttons('Delete server')[0]);

    await vi.waitFor(() => {
      expect(catalog).toEqual([http()]);
    });
    const deleted = sent.find(
      (entry) => entry.method === '_srud/unstable/mcp/delete',
    );
    expect(deleted?.params).toEqual({ name: 'docs' });
    expect(document.body.textContent).not.toContain('my-mcp-server');
  });

  it('keeps the name of a server being edited, since tools are named after it', async () => {
    catalog = [stdio(), http()];
    await render();

    await click(row('docs'));
    await click(button('Settings'));

    expect(inputByLabel('Name').disabled).toBe(true);
    expect(document.body.textContent).toContain(
      'Delete the server to rename it',
    );
  });

  it('shows an edit to the server the pane is showing', async () => {
    catalog = [stdio(), http()];
    await render();

    await click(row('docs'));
    await click(button('Settings'));
    await type(inputByLabel('Command'), 'renamed-server');
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(lastSet()).toEqual(stdio({ command: 'renamed-server' }));
    });
    expect(document.body.textContent).toContain('renamed-server');
  });
});

describe('a refusal from the agent', () => {
  it('is shown in the pane, in this build’s words when the rule is known', async () => {
    await render();
    refuseNext = {
      method: '_srud/unstable/mcp/set',
      error: {
        code: -32602,
        message: '`docs` is listed twice',
        data: { kind: 'duplicate-name', params: { name: 'docs' } },
      },
    };

    await click(button('Add server'));
    await type(inputByLabel('Name'), 'docs');
    await type(inputByLabel('Command'), 'run');
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(document.body.textContent).toContain(
        'Another server is already called',
      );
    });
    expect(catalog).toEqual([]);
  });
});

describe('pasting a configuration', () => {
  const PASTED = `{
    "mcpServers": {
      "context7": {
        "command": "npx",
        "args": ["-y", "@upstash/context7-mcp"]
      }
    }
  }`;

  it('sends every server of it in one call, and says what it added', async () => {
    await render();

    await click(button('Paste JSON'));
    await paste(PASTED);
    expect(document.body.textContent).toContain('Adds: context7');

    await click(button('Import'));

    await vi.waitFor(() => {
      expect(catalog).toEqual([
        stdio({
          name: 'context7',
          command: 'npx',
          args: ['-y', '@upstash/context7-mcp'],
        }),
      ]);
    });
    const imported = sent.find(
      (entry) => entry.method === '_srud/unstable/mcp/import',
    );
    expect(imported?.params.servers).toEqual([
      stdio({
        name: 'context7',
        command: 'npx',
        args: ['-y', '@upstash/context7-mcp'],
      }),
    ]);
    expect(document.body.textContent).toContain('Added context7.');
  });

  it('says when a name it carries is one already configured', async () => {
    catalog = [stdio()];
    await render();

    await click(button('Paste JSON'));
    await paste('{"docs": {"command": "other"}}');

    expect(document.body.textContent).toContain(
      'Replaces the server already called docs',
    );
  });

  it('closes again when the same button is pressed a second time', async () => {
    await render();

    await click(button('Paste JSON'));
    expect(document.querySelector('textarea')).not.toBeNull();

    await click(button('Paste JSON'));
    expect(document.querySelector('textarea')).toBeNull();
    expect(document.body.textContent).toContain('Select a server to edit it.');
  });

  it('will not import text it cannot read', async () => {
    await render();

    await click(button('Paste JSON'));
    await paste('{"mcpServers": ');

    expect(document.body.textContent).toContain('That is not valid JSON.');
    expect(button('Import').disabled).toBe(true);
    expect(
      sent.filter((entry) => entry.method === '_srud/unstable/mcp/import'),
    ).toEqual([]);
  });
});

describe('what a server gives', () => {
  it('is the tools it declares, asked of the server and not of the file', async () => {
    catalog = [stdio()];
    tools = [
      { name: 'echo', description: 'Echo the text back' },
      { name: 'add', description: 'Add two numbers' },
    ];
    await render();

    await click(row('docs'));

    await vi.waitFor(() => {
      expect(document.body.textContent).toContain('Echo the text back');
    });
    expect(document.body.textContent).toContain('echo');
    expect(document.body.textContent).toContain('2 tools');
    // The configuration is what was sent, so a server can be looked at before it
    // is saved — and what is listed is what that command answers now.
    expect(
      sent.find((entry) => entry.method === '_srud/unstable/mcp/tools')?.params,
    ).toEqual({ server: stdio() });
  });

  it('says why a server could not be reached, and asks again when told to', async () => {
    catalog = [stdio()];
    toolsFailure = 'MCP server `docs` could not be reached: spawn failed';
    await render();

    await click(row('docs'));
    await vi.waitFor(() => {
      expect(document.body.textContent).toContain('could not be reached');
    });

    toolsFailure = null;
    tools = [{ name: 'echo', description: 'Echo the text back' }];
    await click(button('List the tools again'));

    await vi.waitFor(() => {
      expect(document.body.textContent).toContain('Echo the text back');
    });
    expect(document.body.textContent).toContain('1 tool');
  });
});
