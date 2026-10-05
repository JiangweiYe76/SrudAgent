// @vitest-environment jsdom
//
// The store reads storage when the module loads, so the loading cases are checked
// by importing it again with the value already in place. Setting the store by
// hand would skip the reading, which is the part that can be wrong.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyZoom, useZoomStore } from './zoom';

beforeEach(() => {
  localStorage.clear();
  document.documentElement.style.zoom = '';
  vi.resetModules();
});

/** Loads the module as if the app were starting with `stored` already saved. */
async function levelAtStartup(stored: string | null): Promise<number> {
  if (stored !== null) localStorage.setItem('srud.zoom', stored);
  const fresh = await import('./zoom');
  return fresh.useZoomStore.getState().level;
}

describe('loading', () => {
  it('takes the level that was chosen', async () => {
    expect(await levelAtStartup('1.5')).toBe(1.5);
  });

  it('falls back to 1 when nothing was saved', async () => {
    expect(await levelAtStartup(null)).toBe(1);
  });

  it('falls back to 1 for a level that was never offered', async () => {
    // 1.1 sits between 1 and 1.25, so no button could show it as chosen. Honouring
    // it would put the settings in a state they cannot represent.
    expect(await levelAtStartup('1.1')).toBe(1);
    expect(await levelAtStartup('nonsense')).toBe(1);
  });
});

describe('setLevel', () => {
  it('keeps the level, writes it down, and scales the window', () => {
    // The three together, because the caller of `setLevel` — the settings row —
    // has no reason to know a separate step is needed to make the change visible.
    useZoomStore.getState().setLevel(2);
    expect(useZoomStore.getState().level).toBe(2);
    expect(localStorage.getItem('srud.zoom')).toBe('2');
    expect(document.documentElement.style.zoom).toBe('2');
  });
});

describe('applyZoom', () => {
  // `main.tsx` calls this one at startup rather than going through the store, so
  // it is the only thing standing between a saved level and the first paint.
  it('scales the root element', () => {
    applyZoom(1.25);
    expect(document.documentElement.style.zoom).toBe('1.25');
  });
});