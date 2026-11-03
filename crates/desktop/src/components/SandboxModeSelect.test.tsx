// @vitest-environment jsdom
//
// Only the trigger is asserted: opening a Radix menu needs a pointer event the
// plain DOM cannot fake. The trigger is what the composer shows, and the menu's
// rows are `choices` rendered through.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { SandboxModeSelect } from './SandboxModeSelect';
import type { SandboxModeOptions } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

const FULL_ACCESS: SandboxModeOptions = {
  currentValue: 'full-access',
  choices: [{ value: 'full-access', name: 'Full access' }],
};

function render(options: SandboxModeOptions | null): void {
  act(() => {
    root.render(<SandboxModeSelect options={options} />);
  });
}

/** The trigger, which is the only button the component renders. */
function trigger(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>('button');
}

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

describe('SandboxModeSelect', () => {
  it('shows the active mode in the destructive colour', () => {
    mount();
    render(FULL_ACCESS);
    expect(trigger()?.textContent).toBe('Full access');
    expect(trigger()?.className).toContain('text-destructive');
  });

  it('shows nothing when the agent advertised no mode', () => {
    mount();
    render(null);
    expect(trigger()).toBeNull();
    render({ currentValue: 'full-access', choices: [] });
    expect(trigger()).toBeNull();
  });
});
