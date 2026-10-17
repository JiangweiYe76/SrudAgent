// @vitest-environment jsdom
//
// The models of one provider, in the dialog that comes second: the provider is
// added, and its models are added to it. What the test pins is the write — the
// whole provider travels back, and the key must not travel with it, or a save
// that only meant to add a model would replace the credential with its mask.
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { Modal, ModalContent } from '@/components/ui/modal';
import type { Provider } from '@/lib/acp';

const mocks = vi.hoisted(() => ({
  invoke: async (_cmd: string, _args?: Record<string, unknown>): Promise<unknown> => {
    throw new Error('invoke not configured');
  },
}));

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: Record<string, unknown>) => mocks.invoke(cmd, args),
}));

vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

const PROVIDER: Provider = {
  id: 'openai',
  name: 'OpenAI',
  baseUrl: 'https://api.example/v1',
  apiKey: 'sk-…1234',
  hasKey: true,
  models: [
    { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
    { id: 'gpt-4o-mini', modalities: [] },
  ],
};

const CATALOG = { providers: [PROVIDER], default: null };
const reply = (id: number, result: unknown) => ({ jsonrpc: '2.0', id, result });

let requests: Array<{ method: string; params: Record<string, unknown> }> = [];
let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.resetModules();
  requests = [];
  mocks.invoke = async (cmd, args) => {
    if (cmd !== 'rpc_request') throw new Error(`unexpected invoke: ${cmd}`);
    const payload = args!.payload as {
      id: number;
      method: string;
      params: Record<string, unknown>;
    };
    requests.push({ method: payload.method, params: payload.params });
    return reply(payload.id, CATALOG);
  };
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function render(provider: Provider = PROVIDER): Promise<void> {
  const { ModelsModal } = await import('./ModelsModal');
  await act(async () => {
    // Inside the settings dialog, which is where it opens in the app.
    root.render(
      <Modal open>
        <ModalContent title="Settings">
          <ModelsModal provider={provider} onClose={() => {}} />
        </ModalContent>
      </Modal>,
    );
  });
}

function matches(button: HTMLButtonElement, label: string): boolean {
  return button.getAttribute('aria-label') === label || button.textContent?.trim() === label;
}

function button(label: string): HTMLButtonElement {
  const found = [...document.querySelectorAll<HTMLButtonElement>('button')].find((b) =>
    matches(b, label),
  );
  if (!found) throw new Error(`no button labelled ${label}`);
  return found;
}

function topDialog(): HTMLElement {
  const dialogs = [...document.querySelectorAll<HTMLElement>('[role="dialog"]')];
  const top = dialogs[dialogs.length - 1];
  if (!top) throw new Error('no dialog is open');
  return top;
}

function topButton(label: string): HTMLButtonElement {
  const found = [...topDialog().querySelectorAll<HTMLButtonElement>('button')].find((b) =>
    matches(b, label),
  );
  if (!found) throw new Error(`no button labelled ${label} in the top dialog`);
  return found;
}

async function click(element: HTMLButtonElement): Promise<void> {
  await act(async () => {
    element.click();
  });
}

function inputByLabel(label: string): HTMLInputElement {
  const found = [...document.querySelectorAll('label')]
    .find((l) => l.querySelector('span')?.textContent === label)
    ?.querySelector('input');
  if (!found) throw new Error(`no field labelled ${label}`);
  return found;
}

async function type(element: HTMLInputElement, value: string): Promise<void> {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  await act(async () => {
    set.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

/** What the model rows currently show, top to bottom. */
function modelRows(): string[] {
  return [...topDialog().querySelectorAll('li')].map(
    (row) => row.querySelector('span')?.textContent ?? '',
  );
}

function setRequest(): { provider: Record<string, unknown> } {
  const found = requests.find((r) => r.method === '_srud/unstable/provider/set');
  if (!found) throw new Error('no write was sent');
  return found.params as { provider: Record<string, unknown> };
}

describe('ModelsModal', () => {
  it('lists the models the provider offers', async () => {
    await render();
    // The label where there is one, and the id where the id is all there is.
    expect(modelRows()).toEqual(['GPT-4o', 'gpt-4o-mini']);
    expect(document.body.textContent).toContain('Models of OpenAI');
  });

  it('adds a model through its own dialog, one layer further in', async () => {
    await render();
    await click(button('Add model'));

    // The third dialog. It has to render, and its fields have to be the ones the
    // typing reaches.
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(3);
    await type(inputByLabel('Model id'), 'o3');
    await type(inputByLabel('Label'), 'o3');
    await click(topButton('Save'));

    expect(modelRows()).toEqual(['GPT-4o', 'gpt-4o-mini', 'o3']);
    // Back to two dialogs: the model dialog is what closed.
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(2);
  });

  it('changes a model in place rather than appending a second one', async () => {
    await render();
    // The row itself is the handle, so a model is edited where it sits.
    const row = topDialog().querySelector('li button');
    if (!(row instanceof HTMLButtonElement)) throw new Error('no model row to click');
    await act(async () => {
      row.click();
    });

    expect(document.body.textContent).toContain('Editing gpt-4o');
    await type(inputByLabel('Model id'), 'gpt-4.1');
    await type(inputByLabel('Label'), 'GPT-4.1');
    await click(topButton('Save'));

    expect(modelRows()).toEqual(['GPT-4.1', 'gpt-4o-mini']);
  });

  it('refuses a model id the provider already offers', async () => {
    await render();
    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'gpt-4o');

    // The agent would refuse the whole provider over this, so it is caught here,
    // next to the field, while it is still one field's problem.
    expect(document.body.textContent).toContain('already offers `gpt-4o`');
    expect(topButton('Save').disabled).toBe(true);
  });

  it('refuses a model id carrying a slash', async () => {
    await render();
    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'openai/gpt-4o');

    expect(document.body.textContent).toContain('must not contain `/`');
    expect(topButton('Save').disabled).toBe(true);
  });

  it('removes a model without opening anything', async () => {
    await render();
    const remove = [...topDialog().querySelectorAll<HTMLButtonElement>('button')].find(
      (b) => b.getAttribute('aria-label') === 'Remove model',
    );
    if (!remove) throw new Error('no remove button');
    await click(remove);

    expect(modelRows()).toEqual(['gpt-4o-mini']);
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(2);
  });

  it('takes a change back when the model dialog is cancelled', async () => {
    await render();
    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'o3');
    await click(topButton('Cancel'));

    expect(modelRows()).toEqual(['GPT-4o', 'gpt-4o-mini']);
  });

  it('writes the provider back with the models and without the key', async () => {
    await render();
    const remove = [...topDialog().querySelectorAll<HTMLButtonElement>('button')].find(
      (b) => b.getAttribute('aria-label') === 'Remove model',
    );
    if (!remove) throw new Error('no remove button');
    await click(remove);
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    expect(setRequest().provider).toEqual({
      id: 'openai',
      name: 'OpenAI',
      baseUrl: 'https://api.example/v1',
      models: [{ id: 'gpt-4o-mini', modalities: [] }],
    });
    // Absent, not the mask: absent is what "keep what is stored" means, and this
    // dialog was not editing the key at all.
    expect(setRequest().provider.apiKey).toBeUndefined();
  });

  it('says what a provider with no model to offer is missing', async () => {
    await render({ ...PROVIDER, models: [] });
    expect(modelRows()).toEqual([]);
    expect(document.body.textContent).toContain('cannot be used');
  });

  it('records what a model accepts besides text', async () => {
    await render();
    await click(button('Add model'));
    await type(inputByLabel('Model id'), 'omni');

    // The three inputs are independent toggles, not a radio group: pressing
    // Video must not clear Image.
    const image = topButton('Image');
    const audio = topButton('Audio');
    const video = topButton('Video');
    expect(image.getAttribute('aria-pressed')).toBe('false');

    await click(image);
    await click(video);
    expect(image.getAttribute('aria-pressed')).toBe('true');
    expect(video.getAttribute('aria-pressed')).toBe('true');
    expect(audio.getAttribute('aria-pressed')).toBe('false');

    // Pressing one twice takes it back off.
    await click(video);
    expect(video.getAttribute('aria-pressed')).toBe('false');

    await click(topButton('Save'));
    await click(button('Save'));

    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    expect(setRequest().provider.models).toEqual([
      { id: 'gpt-4o', name: 'GPT-4o', modalities: [] },
      { id: 'gpt-4o-mini', modalities: [] },
      { id: 'omni', modalities: ['image'] },
    ]);
  });

  it('reopens a model with the modalities it was saved with', async () => {
    await render({
      ...PROVIDER,
      models: [{ id: 'omni', modalities: ['image', 'audio'] }],
    });

    const row = topDialog().querySelector('li button');
    if (!(row instanceof HTMLButtonElement)) throw new Error('no model row to click');
    await act(async () => {
      row.click();
    });

    expect(topButton('Image').getAttribute('aria-pressed')).toBe('true');
    expect(topButton('Audio').getAttribute('aria-pressed')).toBe('true');
    expect(topButton('Video').getAttribute('aria-pressed')).toBe('false');

    // Untouched, the list survives a save in the order the protocol lists it.
    await click(topButton('Save'));
    await click(button('Save'));
    await vi.waitFor(() => {
      expect(requests.some((r) => r.method === '_srud/unstable/provider/set')).toBe(true);
    });
    expect(setRequest().provider.models).toEqual([
      { id: 'omni', modalities: ['image', 'audio'] },
    ]);
  });
});
