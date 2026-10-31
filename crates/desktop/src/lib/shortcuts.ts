import { create } from 'zustand';
import { t } from './i18n';

// A binding is written as `+`-joined modifier names followed by one key, all
// lower-cased: `mod+shift+n`. `mod` is Ctrl everywhere except macOS, where it is
// Command, so one written binding covers both platforms.
//
// Keys a user rebinds are kept in `localStorage`; the defaults below are what a
// fresh install starts from and what "Reset" returns a row to.

export type ShortcutId = 'session.new' | 'settings.open';

export interface ShortcutAction {
  id: ShortcutId;
  // What the action is called in the settings row.
  labelKey: Parameters<typeof t>[0];
  defaultBinding: string;
}

export const SHORTCUT_ACTIONS: ShortcutAction[] = [
  {
    id: 'session.new',
    labelKey: 'shortcuts.session.new',
    defaultBinding: 'mod+n',
  },
  {
    id: 'settings.open',
    labelKey: 'shortcuts.settings.open',
    defaultBinding: 'mod+,',
  },
];

export const DEFAULT_BINDINGS = SHORTCUT_ACTIONS.reduce(
  (all, action) => ({ ...all, [action.id]: action.defaultBinding }),
  {} as Record<ShortcutId, string>,
);

const MODIFIERS = ['mod', 'alt', 'shift'];

// A key that is not a letter or a digit needs a name the binding can hold:
// `+` is the separator, so pressing it has to be spelled out.
const KEY_ALIASES: Record<string, string> = { '+': 'plus' };

// Keys that only ever modify another key. A press of one of these is not a
// binding, it is the user still reaching for one.
const MODIFIER_KEYS = new Set([
  'Shift',
  'Control',
  'Alt',
  'Meta',
  'CapsLock',
  'AltGraph',
  'OS',
]);

export function isMac(): boolean {
  return typeof navigator !== 'undefined' && /mac/i.test(navigator.userAgent);
}

export interface ParsedBinding {
  key: string;
  // In canonical order, so two spellings of the same chord compare equal.
  modifiers: string[];
}

/** Reads a binding, or null when it is not one. */
export function parseBinding(binding: string): ParsedBinding | null {
  const parts = binding.split('+');
  // One modifier and a key at the least: a bare key would fire while the user
  // is typing into something.
  if (parts.length < 2) return null;
  // Lower-cased so a hand-written `mod+P` is the same binding as a recorded one.
  const key = parts[parts.length - 1].toLowerCase();
  if (!key) return null;
  const modifiers = parts.slice(0, -1);
  if (new Set(modifiers).size !== modifiers.length) return null;
  if (!modifiers.every((modifier) => MODIFIERS.includes(modifier))) return null;
  return {
    key,
    modifiers: [...modifiers].sort(
      (a, b) => MODIFIERS.indexOf(a) - MODIFIERS.indexOf(b),
    ),
  };
}

/** The spelling a parsed binding is stored and compared in. */
export function canonicalBinding(binding: string): string | null {
  const parsed = parseBinding(binding);
  return parsed ? [...parsed.modifiers, parsed.key].join('+') : null;
}

/** Reads the chord a press stands for; null for a modifier alone, or a key the platform leaves unnamed. */
export function bindingFromEvent(event: KeyboardEvent): string | null {
  if (MODIFIER_KEYS.has(event.key)) return null;
  const key = event.key.toLowerCase();
  if (key === 'dead' || key === 'unidentified' || key === 'process')
    return null;
  const modifiers: string[] = [];
  if (isMac() ? event.metaKey : event.ctrlKey) modifiers.push('mod');
  if (event.altKey) modifiers.push('alt');
  if (event.shiftKey) modifiers.push('shift');
  return [...modifiers, KEY_ALIASES[event.key] ?? key].join('+');
}

const MAC_SYMBOLS: Record<string, string> = { mod: '⌘', alt: '⌥', shift: '⇧' };
const OTHER_NAMES: Record<string, string> = {
  mod: 'Ctrl',
  alt: 'Alt',
  shift: 'Shift',
};
const KEY_LABELS: Record<string, string> = {
  arrowleft: '←',
  arrowright: '→',
  arrowup: '↑',
  arrowdown: '↓',
  enter: 'Enter',
  escape: 'Esc',
  backspace: 'Backspace',
  delete: 'Del',
  tab: 'Tab',
  space: 'Space',
  plus: '+',
};

/** How a binding reads on screen, in the conventions of the current platform. */
export function formatBinding(binding: string, mac = isMac()): string {
  const parsed = parseBinding(binding);
  if (!parsed) return binding;
  const key = KEY_LABELS[parsed.key] ?? parsed.key.toUpperCase();
  if (mac)
    return (
      parsed.modifiers.map((modifier) => MAC_SYMBOLS[modifier]).join('') + key
    );
  return [
    ...parsed.modifiers.map((modifier) => OTHER_NAMES[modifier]),
    key,
  ].join('+');
}

/** The action already bound to `binding`, if another one is. */
export function conflictFor(
  bindings: Record<ShortcutId, string>,
  id: ShortcutId,
  binding: string,
): ShortcutAction | null {
  const target = canonicalBinding(binding);
  if (!target) return null;
  return (
    SHORTCUT_ACTIONS.find(
      (action) => action.id !== id && bindings[action.id] === target,
    ) ?? null
  );
}

/** The action a key press triggers, or null when it triggers none. */
export function actionForEvent(
  bindings: Record<ShortcutId, string>,
  event: KeyboardEvent,
): ShortcutId | null {
  const binding = bindingFromEvent(event);
  if (!binding) return null;
  return (
    SHORTCUT_ACTIONS.find((action) => bindings[action.id] === binding)?.id ??
    null
  );
}

const STORAGE_KEY = 'srud.shortcuts';

function readStoredBindings(): Record<ShortcutId, string> {
  const bindings = { ...DEFAULT_BINDINGS };
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return bindings;
    const stored = JSON.parse(raw) as Record<string, unknown>;
    for (const action of SHORTCUT_ACTIONS) {
      const value = stored[action.id];
      // Kept only if it still reads as a binding: an entry hand-edited into
      // nonsense would otherwise leave the action with no way to reach it.
      if (typeof value === 'string' && canonicalBinding(value)) {
        bindings[action.id] = canonicalBinding(value) as string;
      }
    }
  } catch {
    // Storage can be full, disabled, or hold something else entirely; defaults
    // are the right answer in every one of those cases.
  }
  return bindings;
}

function persist(bindings: Record<ShortcutId, string>): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(bindings));
  } catch {
    // A refused write costs the binding at the next launch, not this one.
  }
}

interface ShortcutState {
  bindings: Record<ShortcutId, string>;
  // The action whose settings row is capturing keys, if any. The window listener
  // stays out of the way while this is set, so recording a binding does not also
  // fire the one it is replacing.
  recording: ShortcutId | null;
  beginRecording: (id: ShortcutId) => void;
  endRecording: () => void;
  setBinding: (id: ShortcutId, binding: string) => void;
  resetBinding: (id: ShortcutId) => void;
}

export const useShortcutStore = create<ShortcutState>((set, get) => ({
  bindings: readStoredBindings(),
  recording: null,
  beginRecording: (id) => set({ recording: id }),
  endRecording: () => set({ recording: null }),
  setBinding: (id, binding) => {
    const canonical = canonicalBinding(binding);
    if (!canonical) return;
    const bindings = { ...get().bindings, [id]: canonical };
    persist(bindings);
    set({ bindings });
  },
  resetBinding: (id) => {
    const bindings = { ...get().bindings, [id]: DEFAULT_BINDINGS[id] };
    persist(bindings);
    set({ bindings });
  },
}));
