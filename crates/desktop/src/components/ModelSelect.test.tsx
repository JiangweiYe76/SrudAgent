// @vitest-environment jsdom
//
// Only the trigger is asserted: opening a Radix menu needs a pointer event the
// plain DOM cannot fake, and what the menu would show is `groups` rendered
// straight through. The trigger is where the decisions are — which label stands
// for the reference, and whether the session's model is one that still exists.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { ModelSelect } from './ModelSelect';
import { UNAVAILABLE_GROUP } from '@/lib/acp';
import type { ModelOptions } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

function render(options: ModelOptions | null, disabled = false): void {
  act(() => {
    root.render(
      <ModelSelect options={options} disabled={disabled} onSelect={() => {}} />,
    );
  });
}

/** The trigger, which is the only button the component renders. */
function trigger(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('button');
}

const CONFIGURED: ModelOptions = {
  configId: 'model',
  currentValue: 'openai/gpt-4o',
  groups: [
    {
      id: 'openai',
      name: 'OpenAI',
      choices: [
        { value: 'openai/gpt-4o', name: 'GPT-4o' },
        { value: 'openai/gpt-4o-mini', name: 'GPT-4o mini' },
      ],
    },
    {
      id: 'deepseek',
      name: 'DeepSeek',
      choices: [{ value: 'deepseek/chat', name: 'Chat' }],
    },
  ],
};

beforeAll(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function mount(): void {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
}

describe('ModelSelect', () => {
  it('shows the label of the model the session runs on', () => {
    mount();
    render(CONFIGURED);
    expect(trigger()?.textContent).toBe('GPT-4o');
  });

  it('shows nothing when the agent offered no choice', () => {
    mount();
    // A host with one fixed model: an empty picker would claim there was one.
    render(null);
    expect(trigger()).toBeNull();
    render({ configId: 'model', currentValue: '', groups: [] });
    expect(trigger()).toBeNull();
  });

  it('asks for a model when the session has not been given one', () => {
    mount();
    // Nobody has chosen a default, so nothing stands in for one. The trigger is
    // a call to action rather than an empty space where a name would be.
    render({ configId: 'model', currentValue: '', groups: CONFIGURED.groups });

    expect(trigger()?.textContent).toBe('Choose a model');
    expect(trigger()?.className).toContain('text-accent');
  });

  it('falls back to the reference for a model no provider offers', () => {
    mount();
    // The agent keeps the session on the model it named, and adds it as a group
    // of its own. Without a label to show, the reference is what there is.
    render({
      configId: 'model',
      currentValue: 'gone/model',
      groups: [
        ...CONFIGURED.groups,
        {
          id: UNAVAILABLE_GROUP,
          name: 'Unavailable',
          choices: [{ value: 'gone/model', name: 'gone/model' }],
        },
      ],
    });

    expect(trigger()?.textContent).toBe('gone/model');
    expect(trigger()?.className).toContain('text-destructive');
  });

  it('marks an ordinary selection as ordinary', () => {
    mount();
    render(CONFIGURED);
    expect(trigger()?.className).not.toContain('text-destructive');
  });

  it('cannot be opened while a conversation is still arriving', () => {
    mount();
    render(CONFIGURED, true);
    expect(trigger()?.disabled).toBe(true);
  });
});
