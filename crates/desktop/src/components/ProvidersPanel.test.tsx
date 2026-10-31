// @vitest-environment jsdom
//
// The panel is one master-detail surface: the list picks, the pane edits in
// place, and nothing here opens a dialog of its own. It is where a credential
// is typed in, so the test that matters is what a save sends back — a masked
// key read from the agent must not travel back as though it were the key. The
// rest pins the shape of the flow: a provider is added as a provider, its
// models are added after, and the default says so when nobody has chosen one.
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';

const mocks = vi.hoisted(() => ({
  invoke: async (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
    throw new Error('invoke not configured');
  },
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mocks.invoke(cmd, args),
}));

vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

const PROVIDER = {
  id: 'openai',
  name: 'OpenAI',
  baseUrl: 'https://api.example/v1',
  apiKey: 'sk-…1234',
  hasKey: true,
  models: [
    { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
    { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
  ],
};

const reply = (id: number, result: unknown) => ({ jsonrpc: '2.0', id, result });

let requests: Array<{ method: string; params: Record<string, unknown> }> = [];
let catalog: { providers: unknown[]; default: string | null };
let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(async () => {
  vi.resetModules();
  requests = [];
  catalog = { providers: [PROVIDER], default: 'openai/gpt-4o' };
  mocks.invoke = async (cmd, args) => {
    if (cmd !== 'rpc_request') throw new Error(`unexpected invoke: ${cmd}`);
    const payload = args!.payload as {
      id: number;
      method: string;
      params: Record<string, unknown>;
    };
    requests.push({ method: payload.method, params: payload.params });
    switch (payload.method) {
      case '_srud/unstable/provider/list':
      case '_srud/unstable/provider/set':
      case '_srud/unstable/provider/delete':
      case '_srud/unstable/provider/set_default':
        return reply(payload.id, catalog);
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
  const { ProvidersPanel } = await import('./ProvidersPanel');
  await act(async () => {
    root.render(<ProvidersPanel />);
  });
  await vi.waitFor(() => {
    expect(document.body.textContent).toContain('OpenAI');
  });
}

function button(label: string, scope: ParentNode = document.body): HTMLButtonElement {
  const found = [...scope.querySelectorAll<HTMLButtonElement>('button')].find(
    (b) => b.getAttribute('aria-label') === label || b.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no button labelled ${label}`);
  return found;
}

async function click(element: HTMLElement): Promise<void> {
  await act(async () => {
    element.click();
  });
}

// The input of the field labelled `label`. The pane's fields are `<label>`s
// containing their own caption, which is also how the component is built.
function inputByLabel(label: string): HTMLInputElement {
  const found = [...document.querySelectorAll('label')]
    .find((l) => l.querySelector('span')?.textContent === label)
    ?.querySelector('input');
  if (!found) throw new Error(`no field labelled ${label}`);
  return found;
}

// Types into a controlled input the way a browser does: through the native
// value setter, so React's own value tracker sees the change.
async function type(element: HTMLInputElement, value: string): Promise<void> {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  await act(async () => {
    set.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

function defaultTrigger(): HTMLButtonElement {
  const found = button('Default model');
  if (!found) throw new Error('no default model trigger');
  return found;
}

// Radix opens the menu from the keyboard, which a jsdom event can do where a
// pointer sequence cannot.
async function openDefaultMenu(): Promise<void> {
  await act(async () => {
    defaultTrigger().dispatchEvent(
      new KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }),
    );
  });
  await vi.waitFor(() => {
    expect(document.querySelector('[role="menu"]')).not.toBeNull();
  });
}

function menuItem(label: string): HTMLElement {
  const found = [...document.querySelectorAll<HTMLElement>('[role="menuitem"]')].find(
    (item) => item.textContent?.trim() === label,
  );
  if (!found) throw new Error(`no menu item labelled ${label}`);
  return found;
}

function setRequest(): { provider: Record<string, unknown> } {
  const found = requests.find((r) => r.method === '_srud/unstable/provider/set');
  if (!found) throw new Error('no write was sent');
  return found.params as { provider: Record<string, unknown> };
}

/** The detail pane — the right column, where every field and model row lives. */
function detailPane(): HTMLElement {
  const pane = [...document.querySelectorAll('div')].find((d) =>
    d.className.includes('border-l'),
  );
  if (!pane) throw new Error('no detail pane on the page');
  return pane;
}

/** The model rows' edit buttons, the remove buttons filtered out. */
function modelRowButtons(): HTMLButtonElement[] {
  return [...detailPane().querySelectorAll<HTMLButtonElement>('ul li button')].filter(
    (b) => b.getAttribute('aria-label') !== 'Remove model',
  );
}

/** What the detail pane's model rows currently show, top to bottom. */
function modelRows(): string[] {
  return modelRowButtons().map((row) => row.querySelector('span')?.textContent ?? '');
}

describe('ProvidersPanel', () => {
  it('shows the first provider in the pane, and the list keeps the mask', async () => {
    await render();
    // The mask is what the agent handed over, and all this ever sees.
    expect(document.body.textContent).toContain('sk-…1234');
    // The first provider is already being edited: no extra click to get there.
    expect(inputByLabel('Id').value).toBe('openai');
    expect(inputByLabel('Name').value).toBe('OpenAI');
    expect(inputByLabel('Base URL').value).toBe('https://api.example/v1');
  });

  it('says the default is not chosen when nobody has chosen one', async () => {
    catalog = { providers: [PROVIDER], default: null };
    await render();

    // Nothing to inherit, so the trigger stands for nothing rather than for a
    // model the user never picked.
    expect(defaultTrigger().textContent).toBe('Not chosen');
    expect(defaultTrigger().className).toContain('text-muted-foreground');
  });

  it('shows the model a default names', async () => {
    await render();
    expect(defaultTrigger().textContent).toBe('OpenAI · GPT-4o');
    expect(defaultTrigger().className).not.toContain('text-destructive');
  });

  it('marks a default that no provider offers any more', async () => {
    catalog = { providers: [PROVIDER], default: 'gone/model' };
    await render();

    expect(defaultTrigger().textContent).toBe('gone/model (unavailable)');
    expect(defaultTrigger().className).toContain('text-destructive');
  });

  it('writes the chosen model as the default', async () => {
    await render();
    await openDefaultMenu();

    // The model already in force is listed, but picking it again would change
    // nothing.
    expect(menuItem('OpenAI · GPT-4o').getAttribute('data-disabled')).not.toBeNull();
    expect(menuItem('OpenAI · GPT-4o mini').getAttribute('data-disabled')).toBeNull();

    await click(menuItem('OpenAI · GPT-4o mini'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set_default')).toBe(true);
    });
    const request = requests.find((r) => r.method === '_srud/unstable/provider/set_default');
    expect(request?.params).toEqual({ model: 'openai/gpt-4o-mini' });
  });

  it('never writes a masked key back as though it were the key', async () => {
    await render();
    // The key field holds nothing: the mask is where the value would be.
    const key = inputByLabel('API key');
    expect(key.type).toBe('password');
    expect(key.value).toBe('');
    expect(key.placeholder).toBe('sk-…1234');

    await click(button('Save'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    const { provider } = setRequest();
    expect(provider.id).toBe('openai');
    // Absent, not the mask: absent is what "keep what is stored" means.
    expect(provider.apiKey).toBeUndefined();
    // Saving the provider carries its models through untouched.
    expect(provider.models).toEqual([
      { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
      { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
    ]);
    expect(provider.baseUrl).toBe('https://api.example/v1');
  });

  it('adds a provider that has no model yet', async () => {
    await render();
    await click(button('Add provider'));
    expect(inputByLabel('Id').value).toBe('');
    expect(inputByLabel('Name').value).toBe('');

    await type(inputByLabel('Id'), 'local');
    await type(inputByLabel('Name'), 'Local');
    await type(inputByLabel('Base URL'), 'http://localhost:11434/v1');
    await type(inputByLabel('API key'), 'sk-local');
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    expect(setRequest().provider).toEqual({
      id: 'local',
      name: 'Local',
      baseUrl: 'http://localhost:11434/v1',
      apiKey: 'sk-local',
      models: [],
    });
  });

  it('adds a model to a provider that is already there', async () => {
    await render();
    // The list first shows the models the provider offers.
    expect(modelRows()).toEqual(['GPT-4o', 'GPT-4o mini']);

    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'o3');
    await type(inputByLabel('Label'), 'o3');
    await click(button('Save'));
    // The model joined the draft's list, and the pane is back to the provider.
    expect(modelRows()).toEqual(['GPT-4o', 'GPT-4o mini', 'o3']);

    await click(button('Save'));
    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    // The provider travels whole, and its key stays where it is.
    expect(setRequest().provider).toEqual({
      id: 'openai',
      name: 'OpenAI',
      baseUrl: 'https://api.example/v1',
      models: [
        { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
        { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
        { id: 'o3', name: 'o3', modalities: [] },
      ],
    });
    expect(setRequest().provider.apiKey).toBeUndefined();
  });

  it('changes a model in place rather than appending a second one', async () => {
    await render();
    await click(modelRowButtons()[0]);

    expect(document.body.textContent).toContain('Editing gpt-4o');
    await type(inputByLabel('Model id'), 'gpt-4.1');
    await type(inputByLabel('Label'), 'GPT-4.1');
    await click(button('Save'));
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    expect(setRequest().provider.models).toEqual([
      { id: 'gpt-4.1', name: 'GPT-4.1', modalities: [] },
      { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
    ]);
  });

  it('removes a model from the draft without a round trip', async () => {
    await render();
    const remove = [...document.querySelectorAll<HTMLButtonElement>('button')].find(
      (b) => b.getAttribute('aria-label') === 'Remove model',
    );
    if (!remove) throw new Error('no remove button');
    await click(remove);
    expect(modelRows()).toEqual(['GPT-4o mini']);

    await click(button('Save'));
    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    expect(setRequest().provider.models).toEqual([
      { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
    ]);
    expect(setRequest().provider.apiKey).toBeUndefined();
  });

  it('takes a model back when the model form is cancelled', async () => {
    await render();
    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'o3');
    await click(button('Cancel'));

    expect(modelRows()).toEqual(['GPT-4o', 'GPT-4o mini']);
    expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(false);
  });

  it('takes the model form back from its header, discarding what was typed', async () => {
    await render();
    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'o3');
    // The arrow beside the title, visible wherever the scroll has got to.
    await click(button('Back'));

    expect(modelRows()).toEqual(['GPT-4o', 'GPT-4o mini']);
    expect(inputByLabel('Base URL').value).toBe('https://api.example/v1');
    expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(false);
  });

  it('puts the fields back when an edit is cancelled', async () => {
    await render();
    await type(inputByLabel('Name'), 'Renamed');
    await click(button('Cancel'));

    expect(inputByLabel('Name').value).toBe('OpenAI');
    expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(false);
  });

  it('opens a fresh pane for an add, not the one that was just dismissed', async () => {
    await render();
    await type(inputByLabel('Name'), 'Renamed');
    await click(button('Cancel'));

    // An "add" after an "edit" must not inherit so much as a field of it.
    await click(button('Add provider'));
    expect(inputByLabel('Id').value).toBe('');
    expect(inputByLabel('Name').value).toBe('');

    // And cancelling the add returns to the provider that was being shown.
    await click(button('Cancel'));
    expect(inputByLabel('Name').value).toBe('OpenAI');
  });

  it('says what a provider with no model to offer is missing', async () => {
    catalog = { providers: [{ ...PROVIDER, models: [] }], default: null };
    await render();
    expect(document.body.textContent).toContain('cannot be used');
  });
});
