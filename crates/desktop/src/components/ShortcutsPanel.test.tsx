// @vitest-environment jsdom
//
// The settings row is the only place a binding is written, so this covers what
// a user can do there: read the current key, record a new one, be refused when
// it is unusable or taken, back out with Escape, and reset.
import { describe, it, expect, beforeAll, afterEach, beforeEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { ShortcutsPanel } from './ShortcutsPanel';
import { DEFAULT_BINDINGS, useShortcutStore } from '@/lib/shortcuts';

let container: HTMLDivElement;
let root: Root;

function mount(): void {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
}

function render(): void {
  act(() => root.render(<ShortcutsPanel />));
}

/** The binding control of the row named by its action. */
function bindingButton(action: string): HTMLButtonElement {
  return container.querySelector(`button[title="${action}"]`) as HTMLButtonElement;
}

function click(element: HTMLElement): void {
  act(() => element.click());
}

function pressKey(target: Element, init: KeyboardEventInit): void {
  act(() => {
    target.dispatchEvent(new KeyboardEvent('keydown', { bubbles: true, ...init }));
  });
}

beforeAll(() => {
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  localStorage.clear();
  useShortcutStore.setState({ bindings: { ...DEFAULT_BINDINGS }, recording: null });
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe('ShortcutsPanel', () => {
  it('lists every action with the key it answers to', () => {
    mount();
    render();
    expect(bindingButton('New session').textContent).toBe('Ctrl+N');
    expect(bindingButton('Open settings').textContent).toBe('Ctrl+,');
  });

  it('binds the keys pressed while the row listens', () => {
    mount();
    render();
    const control = bindingButton('New session');
    click(control);
    expect(control.textContent).toBe('Press keys…');

    pressKey(control, { key: 'P', ctrlKey: true, shiftKey: true });
    expect(useShortcutStore.getState().bindings['session.new']).toBe('mod+shift+p');
    expect(control.textContent).toBe('Ctrl+Shift+P');
    expect(useShortcutStore.getState().recording).toBeNull();
  });

  it('refuses a key with no modifier on it', () => {
    mount();
    render();
    const control = bindingButton('New session');
    click(control);
    pressKey(control, { key: 'p' });

    expect(useShortcutStore.getState().bindings['session.new']).toBe('mod+n');
    expect(container.textContent).toContain('Hold Ctrl, Alt or Shift');
  });

  it('refuses a chord another action already holds', () => {
    mount();
    render();
    const control = bindingButton('Open settings');
    click(control);
    pressKey(control, { key: 'N', ctrlKey: true });

    expect(useShortcutStore.getState().bindings['settings.open']).toBe('mod+,');
    expect(container.textContent).toContain('Already bound to “New session”.');
  });

  it('leaves the binding as it was when Escape backs out', () => {
    mount();
    render();
    const control = bindingButton('Open settings');
    click(control);
    pressKey(control, { key: 'Escape' });

    expect(useShortcutStore.getState().recording).toBeNull();
    expect(control.textContent).toBe('Ctrl+,');
  });

  it('resets a rebound row to its default', () => {
    useShortcutStore.setState({
      bindings: { ...DEFAULT_BINDINGS, 'session.new': 'mod+alt+n' },
    });
    mount();
    render();
    expect(bindingButton('New session').textContent).toBe('Ctrl+Alt+N');

    const reset = Array.from(container.querySelectorAll('button')).find(
      (element) => element.textContent === 'Reset',
    ) as HTMLButtonElement;
    click(reset);
    expect(useShortcutStore.getState().bindings['session.new']).toBe('mod+n');
  });
});
