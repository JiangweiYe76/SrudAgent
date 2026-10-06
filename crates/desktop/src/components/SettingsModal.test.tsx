// @vitest-environment jsdom
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { SettingsModal } from './SettingsModal';
import { useZoomStore } from '@/lib/zoom';

// The window is scaled by the webview, so that is what the click has to reach.
// Recorded rather than merely stubbed, so the test can say what was asked for.
const mocks = vi.hoisted(() => ({
  setZoom: vi.fn(async (_level: number) => {}),
}));

vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ setZoom: mocks.setZoom }),
}));

let container: HTMLDivElement;
let root: Root;

function render(): void {
  act(() => {
    root.render(<SettingsModal open onClose={() => {}} />);
  });
}

/** The zoom buttons, in the order the settings offer them. */
function zoomButtons(): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>('button[aria-pressed]')].filter((b) =>
    b.textContent?.endsWith('%'),
  );
}

/** Clicks the button labelled with `label`, e.g. `'150%'`. */
async function choose(label: string): Promise<void> {
  await act(async () => {
    zoomButtons().find((b) => b.textContent === label)!.click();
  });
}

/** The pressed state of the button labelled with `label`. */
function isChosen(label: string): string | null {
  return zoomButtons().find((b) => b.textContent === label)!.getAttribute('aria-pressed');
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  localStorage.clear();
  mocks.setZoom.mockClear();
  useZoomStore.setState({ level: 1 });
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe('SettingsModal zoom', () => {
  it('offers every level and marks the current one', () => {
    render();
    expect(zoomButtons().map((b) => b.textContent)).toEqual([
      '80%',
      '100%',
      '125%',
      '150%',
      '200%',
    ]);
    expect(['80%', '100%', '125%', '150%', '200%'].map(isChosen)).toEqual([
      'false',
      'true',
      'false',
      'false',
      'false',
    ]);
  });

  it('scales the window when a level is picked', async () => {
    render();
    await choose('150%');
    expect(mocks.setZoom).toHaveBeenCalledWith(1.5);
    expect(useZoomStore.getState().level).toBe(1.5);
    expect(localStorage.getItem('srud.zoom')).toBe('1.5');
  });

  it('opens on the level that was saved, not on 100%', () => {
    // Buttons that ignored the store would show 100% as chosen while the window sat
    // at 150%, and clicking 150% would then look like it did nothing.
    useZoomStore.setState({ level: 2 });
    render();
    expect(isChosen('200%')).toBe('true');
    expect(isChosen('100%')).toBe('false');
  });
});