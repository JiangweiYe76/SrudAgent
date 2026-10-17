// @vitest-environment jsdom
//
// The form is a dialog, and in the app it sits inside the settings dialog. What
// it holds is four fields: a provider is an endpoint and a credential, and the
// models it offers are added afterwards, from its row in the panel. So the cases
// worth pinning are that it renders where it is put, that it can be saved
// without a model, and that the credential field conceals what it holds until
// asked — a key nobody has chosen to look at should not be on screen.
import { describe, it, expect, vi, beforeAll, beforeEach, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { Modal, ModalContent } from '@/components/ui/modal';
import type { Provider } from '@/lib/acp';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: async () => {
    throw new Error('the form alone makes no calls');
  },
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

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.resetModules();
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

async function renderInSettings(provider: Provider | null): Promise<void> {
  const { ProviderFormModal } = await import('./ProviderFormModal');
  await act(async () => {
    root.render(
      <Modal open>
        <ModalContent title="Settings">
          <ProviderFormModal provider={provider} onClose={() => {}} />
        </ModalContent>
      </Modal>,
    );
  });
}

/** Every button in the document, as the tests address them by label. */
function buttons(scope: ParentNode = document.body): HTMLButtonElement[] {
  return [...scope.querySelectorAll<HTMLButtonElement>('button')];
}

function matches(button: HTMLButtonElement, label: string): boolean {
  return button.getAttribute('aria-label') === label || button.textContent?.trim() === label;
}

function button(label: string): HTMLButtonElement {
  const found = buttons().find((b) => matches(b, label));
  if (!found) throw new Error(`no button labelled ${label}`);
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

// Types into a controlled input the way a browser does: through the native
// value setter, so React's own value tracker sees the change.
async function type(element: HTMLInputElement, value: string): Promise<void> {
  const set = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!;
  await act(async () => {
    set.call(element, value);
    element.dispatchEvent(new Event('input', { bubbles: true }));
  });
}

describe('ProviderFormModal', () => {
  it('renders its fields inside the dialog that holds it', async () => {
    await renderInSettings(PROVIDER);

    // The inner dialog's title, and the outer one's, are both on the page: the
    // inner is not being hidden by the outer's treatment of the rest of the DOM.
    expect(document.body.textContent).toContain('Settings');
    expect(document.body.textContent).toContain('Editing openai');
    expect(document.body.textContent).toContain('Base URL');
    // Id, name, base URL, key — and no model list among them.
    expect(document.querySelectorAll('input')).toHaveLength(4);
  });

  it('holds no models of its own', async () => {
    await renderInSettings(PROVIDER);
    // The list of models belongs to the models dialog. What this says is that
    // there is one, and where.
    expect(document.body.textContent).toContain('offers 2 model(s)');
    expect(document.querySelectorAll('li')).toHaveLength(0);
  });

  it('conceals the key until it is asked to show it', async () => {
    await renderInSettings(PROVIDER);
    const key = inputByLabel('API key');

    expect(key.type).toBe('password');
    expect(key.value).toBe('');

    await click(button('Show value'));
    expect(inputByLabel('API key').type).toBe('text');
    // The label now names what the next press does, not what the last one did.
    await click(button('Hide value'));
    expect(inputByLabel('API key').type).toBe('password');
  });

  it('shows the stored key as a placeholder rather than a value', async () => {
    await renderInSettings(PROVIDER);
    const key = inputByLabel('API key');

    // The mask is where the value would be, greyed, and the field is still
    // empty: anything else would be submitted as the key.
    expect(key.placeholder).toBe('sk-…1234');
    expect(key.value).toBe('');
  });

  it('saves a provider that has no model in it yet', async () => {
    await renderInSettings(null);
    expect(document.body.textContent).toContain('Add a provider');
    // Nothing worth saving yet.
    expect(button('Save').disabled).toBe(true);

    await type(inputByLabel('Id'), 'local');
    await type(inputByLabel('Name'), 'Local');
    await type(inputByLabel('Base URL'), 'http://localhost:11434/v1');

    // Every field filled, and no model anywhere: that is a complete provider.
    expect(button('Save').disabled).toBe(false);
  });

  it('starts an existing provider with its fields, and no key', async () => {
    const { draftOf } = await import('./ProviderFormModal');
    const draft = draftOf(PROVIDER);

    // Absent, not the mask: a form that starts from the display value would
    // save that over the credential the moment the user touches anything else.
    expect(draft.apiKey).toBeUndefined();
    expect(draft.id).toBe('openai');
    expect(draft.name).toBe('OpenAI');
    expect(draft.baseUrl).toBe('https://api.example/v1');
  });

  it('carries the models it does not edit through untouched', async () => {
    const { draftOf } = await import('./ProviderFormModal');
    // A save from here must not read as "and delete them all".
    expect(draftOf(PROVIDER).models).toEqual(PROVIDER.models);
  });

  it('starts an add with nothing filled in', async () => {
    const { draftOf } = await import('./ProviderFormModal');
    expect(draftOf(null)).toEqual({ id: '', name: '', baseUrl: '', apiKey: '', models: [] });
  });
});
