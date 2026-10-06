// @vitest-environment jsdom
//
// The store reads storage when the module loads, so the loading cases are checked
// by importing it again with the value already in place. Setting the store by
// hand would skip the reading, which is the part that can be wrong.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { applyZoom, useZoomStore } from './zoom';

// The zoom is the webview's, so that is what the tests watch. A record of the calls
// rather than a plain stub, so a test can say what was asked for and not merely that
// something was.
const mocks = vi.hoisted(() => ({
  setZoom: vi.fn(async (_level: number) => {}),
}));

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ setZoom: mocks.setZoom }),
}));

beforeEach(() => {
  localStorage.clear();
  mocks.setZoom.mockClear();
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
  it('keeps the level, writes it down, and scales the window', async () => {
    // The three together, because the caller of `setLevel` — the settings row —
    // has no reason to know a separate step is needed to make the change visible.
    await useZoomStore.getState().setLevel(2);
    expect(useZoomStore.getState().level).toBe(2);
    expect(localStorage.getItem('srud.zoom')).toBe('2');
    expect(mocks.setZoom).toHaveBeenCalledWith(2);
  });

  it('does not record a level the window is not at yet', async () => {
    // The webview call is the slow one, and it is the one that decides what the
    // window looks like. Recording first would let the settings and the saved value
    // claim a level that never got applied — which on a slow machine is a visible
    // jump on the next start.
    let release: () => void = () => {};
    mocks.setZoom.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          release = resolve;
        }),
    );

    // The store is a module singleton, so what "not yet" means is whatever it held
    // a moment ago rather than a hardcoded 1.
    const before = useZoomStore.getState().level;

    const pending = useZoomStore.getState().setLevel(1.5);
    expect(useZoomStore.getState().level).toBe(before);
    expect(localStorage.getItem('srud.zoom')).toBeNull();

    release();
    await pending;
    expect(useZoomStore.getState().level).toBe(1.5);
    expect(localStorage.getItem('srud.zoom')).toBe('1.5');
  });
});

describe('applyZoom', () => {
  // `main.tsx` calls this one at startup rather than going through the store, so
  // it is the only thing standing between a saved level and the first paint.
  it('tells the webview the level', async () => {
    await applyZoom(1.25);
    expect(mocks.setZoom).toHaveBeenCalledWith(1.25);
  });

  it('leaves the document unscaled', async () => {
    // The bug this replaced: `zoom` on `<html>` scaled Radix's fixed-position portal
    // a second time, so a dropdown's `translate` came out doubled — measured at 80 px
    // adrift at 125% and 520 px at 200%. jsdom has no layout, so the drift cannot be
    // measured here; what can be is that the one thing that caused it is gone.
    await applyZoom(2);
    expect(document.documentElement.style.zoom).toBe('');
  });

  it('reports a refused zoom instead of throwing it at the caller', async () => {
    // Both callers are places a throw is worse than a wrong-sized window: the first
    // paint would have no app at all, and the settings row would lose the level the
    // user just picked. `main.tsx` awaits this one before it renders.
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mocks.setZoom.mockRejectedValueOnce(new Error('not supported'));

    await expect(applyZoom(1.5)).resolves.toBeUndefined();
    // Reported rather than dropped, so a webview without zoom support shows up here
    // instead of looking like a click that did nothing.
    expect(warn).toHaveBeenCalledOnce();

    warn.mockRestore();
  });

  it('keeps the level the user picked even when the zoom is refused', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    mocks.setZoom.mockRejectedValueOnce(new Error('not supported'));

    await useZoomStore.getState().setLevel(1.5);
    expect(useZoomStore.getState().level).toBe(1.5);
    expect(localStorage.getItem('srud.zoom')).toBe('1.5');

    warn.mockRestore();
  });
});