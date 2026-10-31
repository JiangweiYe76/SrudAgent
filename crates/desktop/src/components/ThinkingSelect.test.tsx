// @vitest-environment jsdom
//
// Only the trigger is asserted, for the same reason as `ModelSelect`: opening a
// Radix menu needs a pointer event the plain DOM cannot fake, and the menu is
// `choices` rendered straight through. The trigger is where the decisions are —
// which level's label stands in for the canonical value, and whether the model
// offers a selector at all.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { ThinkingSelect } from './ThinkingSelect';
import type { ThinkingOptions } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

function render(options: ThinkingOptions | null, disabled = false): void {
  act(() => {
    root.render(<ThinkingSelect options={options} disabled={disabled} onSelect={() => {}} />);
  });
}

/** The trigger, which is the only button the component renders. */
function trigger(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('button');
}

const DECLARED: ThinkingOptions = {
  configId: 'reasoning',
  currentValue: 'medium',
  choices: [
    { value: 'off', name: 'Off' },
    { value: 'medium', name: 'Medium' },
    { value: 'max', name: 'Max' },
  ],
};

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
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

describe('ThinkingSelect', () => {
  it('shows the label of the level the session sends', () => {
    mount();
    render(DECLARED);
    expect(trigger()?.textContent).toBe('Medium');
  });

  it('shows nothing when the model declares no thinking control', () => {
    mount();
    // A model with no levels has nothing to choose; an empty picker would claim
    // otherwise.
    render(null);
    expect(trigger()).toBeNull();
    render({ configId: 'reasoning', currentValue: 'medium', choices: [] });
    expect(trigger()).toBeNull();
  });

  it('falls back to the value when the list does not hold it', () => {
    mount();
    render({ ...DECLARED, currentValue: 'low' });
    expect(trigger()?.textContent).toBe('low');
  });

  it('asks for a choice when the session has not been given one', () => {
    mount();
    render({ ...DECLARED, currentValue: '' });
    expect(trigger()?.textContent).toBe('Thinking');
    expect(trigger()?.className).toContain('text-accent');
  });

  it('cannot be opened while a conversation is still arriving', () => {
    mount();
    render(DECLARED, true);
    expect(trigger()?.disabled).toBe(true);
  });
});
