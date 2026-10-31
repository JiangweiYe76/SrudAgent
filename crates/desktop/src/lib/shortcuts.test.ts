// @vitest-environment jsdom

// Bindings are written, read back, compared and stored here — the settings
// panel and the window listener both trust this module to spell a chord the
// same way twice.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  DEFAULT_BINDINGS,
  actionForEvent,
  bindingFromEvent,
  canonicalBinding,
  conflictFor,
  formatBinding,
  parseBinding,
} from './shortcuts';

const press = (init: KeyboardEventInit) => new KeyboardEvent('keydown', init);

beforeEach(() => {
  localStorage.clear();
  vi.resetModules();
});

describe('shortcut bindings', () => {
  it('spells a press as modifiers and one lower-cased key', () => {
    expect(bindingFromEvent(press({ key: 'N', ctrlKey: true }))).toBe('mod+n');
    expect(
      bindingFromEvent(press({ key: 'N', ctrlKey: true, shiftKey: true })),
    ).toBe('mod+shift+n');
    expect(bindingFromEvent(press({ key: ',', ctrlKey: true }))).toBe('mod+,');
  });

  it('treats a modifier pressed on its own as no binding yet', () => {
    expect(
      bindingFromEvent(press({ key: 'Shift', shiftKey: true })),
    ).toBeNull();
    expect(
      bindingFromEvent(press({ key: 'Control', ctrlKey: true })),
    ).toBeNull();
  });

  it('names the plus key, which would otherwise split the binding', () => {
    const binding = bindingFromEvent(
      press({ key: '+', ctrlKey: true, shiftKey: true }),
    );
    expect(binding).toBe('mod+shift+plus');
    expect(parseBinding(binding as string)?.key).toBe('plus');
  });

  it('reads a binding, putting its modifiers in one order', () => {
    expect(parseBinding('shift+mod+n')).toEqual({
      key: 'n',
      modifiers: ['mod', 'shift'],
    });
    expect(canonicalBinding('shift+mod+n')).toBe('mod+shift+n');
  });

  it('refuses what is not a binding', () => {
    // A bare key would fire while the user types into something.
    expect(parseBinding('n')).toBeNull();
    expect(parseBinding('')).toBeNull();
    expect(parseBinding('mod+')).toBeNull();
    expect(parseBinding('mod+mod+n')).toBeNull();
    // Not a modifier this app knows.
    expect(parseBinding('hyper+n')).toBeNull();
  });

  it('shows a binding in the conventions of the platform', () => {
    expect(formatBinding('mod+shift+n', false)).toBe('Ctrl+Shift+N');
    expect(formatBinding('mod+shift+n', true)).toBe('⌘⇧N');
    expect(formatBinding('mod+,', false)).toBe('Ctrl+,');
    expect(formatBinding('alt+arrowleft', false)).toBe('Alt+←');
  });

  it('finds the action a press triggers, and nothing for the rest', () => {
    expect(
      actionForEvent(DEFAULT_BINDINGS, press({ key: 'n', ctrlKey: true })),
    ).toBe('session.new');
    expect(
      actionForEvent(DEFAULT_BINDINGS, press({ key: ',', ctrlKey: true })),
    ).toBe('settings.open');
    expect(
      actionForEvent(DEFAULT_BINDINGS, press({ key: 'x', ctrlKey: true })),
    ).toBeNull();
  });

  it('finds the other action holding a binding, and not the row itself', () => {
    expect(conflictFor(DEFAULT_BINDINGS, 'settings.open', 'mod+n')?.id).toBe(
      'session.new',
    );
    expect(conflictFor(DEFAULT_BINDINGS, 'session.new', 'mod+n')).toBeNull();
    expect(
      conflictFor(DEFAULT_BINDINGS, 'settings.open', 'mod+alt+p'),
    ).toBeNull();
  });
});

describe('shortcut store', () => {
  it('keeps a rebound key, spelled one way, and stores it', async () => {
    const { useShortcutStore } = await import('./shortcuts');
    useShortcutStore.getState().setBinding('session.new', 'shift+mod+P');
    expect(useShortcutStore.getState().bindings['session.new']).toBe(
      'mod+shift+p',
    );
    expect(
      JSON.parse(localStorage.getItem('srud.shortcuts') as string),
    ).toMatchObject({
      'session.new': 'mod+shift+p',
    });
  });

  it('refuses a binding it cannot parse', async () => {
    const { useShortcutStore } = await import('./shortcuts');
    useShortcutStore.getState().setBinding('session.new', 'p');
    expect(useShortcutStore.getState().bindings['session.new']).toBe(
      DEFAULT_BINDINGS['session.new'],
    );
  });

  it('resets one action to its default', async () => {
    const { useShortcutStore } = await import('./shortcuts');
    useShortcutStore.getState().setBinding('session.new', 'mod+alt+n');
    useShortcutStore.getState().resetBinding('session.new');
    expect(useShortcutStore.getState().bindings['session.new']).toBe('mod+n');
  });

  it('starts from what was stored, ignoring entries that no longer read', async () => {
    localStorage.setItem(
      'srud.shortcuts',
      JSON.stringify({
        'session.new': 'mod+alt+n',
        'settings.open': 'nonsense',
      }),
    );
    const { useShortcutStore } = await import('./shortcuts');
    const bindings = useShortcutStore.getState().bindings;
    expect(bindings['session.new']).toBe('mod+alt+n');
    // Fell back rather than leaving the action with no way to reach it.
    expect(bindings['settings.open']).toBe(DEFAULT_BINDINGS['settings.open']);
  });
});
