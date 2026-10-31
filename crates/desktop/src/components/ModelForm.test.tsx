// @vitest-environment jsdom
//
// The model form is a pane inside the provider detail, so it makes no calls of
// its own: what it produces is the object handed to onSave. The cases worth
// pinning are the agent's own refusal rules caught next to the field, what a
// declaration means — an empty level list is a model with no thinking control —
// and that a model reopens with what it was saved with.
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
import type { ProviderModel } from '@/lib/acp';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: async () => {
    throw new Error('the form alone makes no calls');
  },
}));

vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }));

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
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

let onSave: (model: ProviderModel) => void;
let onCancel: () => void;

async function renderForm(
  model: ProviderModel | null,
  taken: string[] = [],
): Promise<void> {
  const { ModelForm } = await import('./ModelForm');
  onSave = vi.fn<(model: ProviderModel) => void>();
  onCancel = vi.fn<() => void>();
  await act(async () => {
    root.render(
      <ModelForm
        model={model}
        taken={taken}
        onSave={onSave}
        onCancel={onCancel}
      />,
    );
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

// The trigger of a select control, which is a menu button rather than a native
// `<select>`.
function selectTrigger(label: string): HTMLButtonElement {
  const found = document.querySelector<HTMLButtonElement>(
    `button[aria-label="${label}"]`,
  );
  if (!found) throw new Error(`no select labelled ${label}`);
  return found;
}

function inputByPlaceholder(placeholder: string): HTMLInputElement {
  const found = [...document.querySelectorAll<HTMLInputElement>('input')].find(
    (input) => input.placeholder === placeholder,
  );
  if (!found) throw new Error(`no field with placeholder ${placeholder}`);
  return found;
}

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

describe('ModelForm', () => {
  it('saves a bare model with nothing but an id', async () => {
    await renderForm(null);
    await type(inputByLabel('Model id'), 'o3');
    await click(button('Save'));

    expect(onSave).toHaveBeenCalledWith({ id: 'o3', modalities: [] });
  });

  it('refuses a model id the provider already offers', async () => {
    await renderForm(null, ['gpt-4o']);
    await type(inputByLabel('Model id'), 'gpt-4o');

    // The agent would refuse the whole provider over this, so it is caught here,
    // next to the field, while it is still one field's problem.
    expect(document.body.textContent).toContain('already offers `gpt-4o`');
    expect(button('Save').disabled).toBe(true);
  });

  it('refuses a model id carrying a slash', async () => {
    await renderForm(null);
    await type(inputByLabel('Model id'), 'openai/gpt-4o');

    expect(document.body.textContent).toContain('must not contain `/`');
    expect(button('Save').disabled).toBe(true);
  });

  it('records what a model accepts besides text', async () => {
    await renderForm(null);
    await type(inputByLabel('Model id'), 'omni');

    // The three inputs are independent toggles, not a radio group: pressing
    // Video must not clear Image.
    const image = button('Image');
    const audio = button('Audio');
    const video = button('Video');
    expect(image.getAttribute('aria-pressed')).toBe('false');

    await click(image);
    await click(video);
    expect(image.getAttribute('aria-pressed')).toBe('true');
    expect(video.getAttribute('aria-pressed')).toBe('true');
    expect(audio.getAttribute('aria-pressed')).toBe('false');

    // Pressing one twice takes it back off.
    await click(video);
    expect(video.getAttribute('aria-pressed')).toBe('false');

    await click(button('Save'));
    expect(onSave).toHaveBeenCalledWith({ id: 'omni', modalities: ['image'] });
  });

  it('reopens a model with the modalities it was saved with', async () => {
    await renderForm({ id: 'omni', modalities: ['image', 'audio'] });

    expect(button('Image').getAttribute('aria-pressed')).toBe('true');
    expect(button('Audio').getAttribute('aria-pressed')).toBe('true');
    expect(button('Video').getAttribute('aria-pressed')).toBe('false');

    // Untouched, the list survives a save in the order the protocol lists it.
    await click(button('Save'));
    expect(onSave).toHaveBeenCalledWith({
      id: 'omni',
      modalities: ['image', 'audio'],
    });
  });

  it('records the thinking levels a model was configured with', async () => {
    await renderForm(null);
    await type(inputByLabel('Model id'), 'kimi');

    // An empty list is an ordinary chat model: no thinking control, and a
    // model saved without a declaration. It is not a form error.
    expect(button('Save').disabled).toBe(false);

    await click(button('Add level'));
    // The first row is `off`, and it becomes the default.
    expect(selectTrigger('Default level').textContent).toBe('Off');
    // A level with no wire value is still not a declaration.
    expect(button('Save').disabled).toBe(true);

    await type(inputByPlaceholder('off'), 'none');
    expect(button('Save').disabled).toBe(false);

    await click(button('Save'));
    expect(onSave).toHaveBeenCalledWith({
      id: 'kimi',
      modalities: [],
      thinking: { default: 'off', levels: { off: 'none' } },
    });
  });

  it('reopens a model with the thinking it was saved with', async () => {
    await renderForm({
      id: 'kimi',
      modalities: [],
      thinking: {
        default: 'max',
        levels: { off: 'false', low: 'low', max: 'max' },
      },
    });

    // Rows are listed weakest to strongest, whatever order they were stored in.
    expect(
      [
        ...document.querySelectorAll<HTMLButtonElement>(
          'button[aria-label="Level"]',
        ),
      ][0].textContent,
    ).toBe('Off');
    expect(selectTrigger('Default level').textContent).toBe('Max');

    // Untouched, the declaration survives a save as it was.
    await click(button('Save'));
    expect(onSave).toHaveBeenCalledWith({
      id: 'kimi',
      modalities: [],
      thinking: {
        default: 'max',
        levels: { off: 'false', low: 'low', max: 'max' },
      },
    });
  });

  it('drops a thinking declaration when every level is removed', async () => {
    await renderForm({
      id: 'kimi',
      modalities: [],
      thinking: { default: 'low', levels: { low: 'low' } },
    });

    // No levels is the form's only way to say "this model has no thinking
    // control"; there is no separate on/off switch.
    await click(button('Remove level'));
    expect(
      [...document.querySelectorAll('label')].some(
        (label) => label.querySelector('span')?.textContent === 'Default level',
      ),
    ).toBe(false);
    await click(button('Save'));
    // Absent, not an empty declaration: a model with no thinking control is
    // what an empty level list means to the agent.
    expect(onSave).toHaveBeenCalledWith({ id: 'kimi', modalities: [] });
  });

  it('takes the fields back when cancelled', async () => {
    await renderForm({ id: 'kimi', modalities: [] });
    await type(inputByLabel('Model id'), 'changed');
    await click(button('Cancel'));

    expect(onCancel).toHaveBeenCalled();
    expect(onSave).not.toHaveBeenCalled();
  });
});
