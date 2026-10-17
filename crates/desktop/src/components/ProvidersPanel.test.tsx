// @vitest-environment jsdom
//
// The panel is a list and two dialogs, and it is where a credential is typed in,
// so the test that matters is what a save sends back: a masked key read from the
// agent must not travel back as though it were the key. The rest pins the shape
// of the flow — a provider is added as a provider, its models are added after,
// and the default says so when nobody has chosen one.
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
    { id: 'gpt-4o', name: 'GPT-4o' },
    { id: 'gpt-4o-mini', name: 'GPT-4o mini' },
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
  const React = await import('react');
  const { ProvidersPanel } = await import('./ProvidersPanel');
  const { ProviderFormModal } = await import('./ProviderFormModal');
  const { ModelsModal } = await import('./ModelsModal');
  const useState = React.useState;
  function Wrapper() {
    const [form, setForm] = useState<any>(null);
    const [modelsFor, setModelsFor] = useState<any>(null);
    return React.createElement(
      React.Fragment,
      null,
      React.createElement(ProvidersPanel, {
        form,
        onFormChange: setForm,
        modelsFor,
        onModelsForChange: setModelsFor,
      }),
      form
        ? React.createElement(ProviderFormModal, {
            provider: form.provider,
            onClose: () => setForm(null),
          })
        : null,
      modelsFor
        ? React.createElement(ModelsModal, {
            provider: modelsFor,
            onClose: () => setModelsFor(null),
          })
        : null,
    );
  }
  await act(async () => {
    root.render(React.createElement(Wrapper));
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

async function click(element: HTMLButtonElement): Promise<void> {
  await act(async () => {
    element.click();
  });
}

// The input of the field labelled `label`. The form's fields are `<label>`s
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

/** The dialog on top — the one the user is actually looking at. */
function topButton(label: string): HTMLButtonElement {
  const dialogs = [...document.querySelectorAll<HTMLElement>('[role="dialog"]')];
  const top = dialogs[dialogs.length - 1];
  if (!top) throw new Error('no dialog is open');
  return button(label, top);
}

function defaultSelect(): HTMLSelectElement {
  const found = document.querySelector('select');
  if (!found) throw new Error('no default model select');
  return found;
}

function setRequest(): { provider: Record<string, unknown> } {
  const found = requests.find((r) => r.method === '_srud/unstable/provider/set');
  if (!found) throw new Error('no write was sent');
  return found.params as { provider: Record<string, unknown> };
}

describe('ProvidersPanel', () => {
  it('lists the configured providers with the key masked', async () => {
    await render();
    expect(document.body.textContent).toContain('OpenAI');
    // The mask is what the agent handed over, and all this ever sees.
    expect(document.body.textContent).toContain('sk-…1234');
    // The model count belongs to the button that opens them.
    expect(button('Manage models').textContent).toContain('2 model(s)');
  });

  it('says the default is not chosen when nobody has chosen one', async () => {
    catalog = { providers: [PROVIDER], default: null };
    await render();

    expect(defaultSelect().value).toBe('');
    expect(defaultSelect().selectedOptions[0].textContent).toBe('Not chosen');
    // Nothing to inherit, so the option stands for nothing rather than for a
    // model the user never picked.
    expect(defaultSelect().selectedOptions[0].disabled).toBe(true);
  });

  it('shows the model a default names', async () => {
    await render();
    expect(defaultSelect().value).toBe('openai/gpt-4o');
    expect(defaultSelect().selectedOptions[0].textContent).toBe('OpenAI · GPT-4o');
  });

  it('never writes a masked key back as though it were the key', async () => {
    await render();
    await click(button('Edit provider'));
    expect(document.body.textContent).toContain('Editing openai');
    // Nothing typed: the form holds the provider as the agent reported it.
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    const { provider } = setRequest();
    expect(provider.id).toBe('openai');
    // Absent, not the mask: absent is what "keep what is stored" means.
    expect(provider.apiKey).toBeUndefined();
    // Saving the provider does not touch the models it is not showing.
    expect(provider.models).toEqual([
      { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
      { id: 'gpt-4o-mini', name: 'GPT-4o mini', modalities: [] },
    ]);
    expect(provider.baseUrl).toBe('https://api.example/v1');
  });

  it('closes the form once a save lands', async () => {
    await render();
    await click(button('Edit provider'));
    expect(document.body.textContent).toContain('Editing openai');

    await click(button('Save'));

    await vi.waitFor(() => {
      expect(document.body.textContent).not.toContain('Editing openai');
    });
  });

  it('adds a provider that has no model yet', async () => {
    await render();
    await click(button('Add provider'));
    expect(document.body.textContent).toContain('Add a provider');

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
    await click(button('Manage models'));
    expect(document.body.textContent).toContain('Models of OpenAI');

    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'o3');
    await type(inputByLabel('Label'), 'o3');
    // The model dialog is the top one, so its Save is the one that closes it.
    await click(topButton('Save'));
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

  it('writes nothing when the form is cancelled', async () => {
    await render();
    await click(button('Edit provider'));
    await type(inputByLabel('Name'), 'Renamed');

    await click(button('Cancel'));

    expect(document.body.textContent).not.toContain('Editing openai');
    expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(false);
  });

  it('opens a fresh form, not the one that was just dismissed', async () => {
    await render();
    await click(button('Edit provider'));
    await type(inputByLabel('Name'), 'Renamed');
    await click(button('Cancel'));

    // An "add" after an "edit" must not inherit so much as a field of it.
    await click(button('Add provider'));
    expect(inputByLabel('Id').value).toBe('');
    expect(inputByLabel('Name').value).toBe('');
  });
});
