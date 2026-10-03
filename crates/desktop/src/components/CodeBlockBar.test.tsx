// @vitest-environment jsdom
//
// The copy button's behaviour is timing-dependent, so it is checked with a fake
// clock rather than by waiting: a real 1.5s wait per case would make the suite
// slower than the thing it tests, and would still not catch a timer that fires
// early.
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { CopyCode } from './CodeBlockBar';

let container: HTMLDivElement;
let root: Root;
let writeText: ReturnType<typeof vi.fn>;

/** Renders the button and returns it. */
function button(): HTMLButtonElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<CopyCode text="graph LR" />);
  });
  return container.querySelector('button')!;
}

/** Clicks and lets the write's promise settle. */
async function click(): Promise<void> {
  await act(async () => {
    container.querySelector('button')!.click();
  });
}

/** The label the button is currently offering. */
function title(): string | null {
  return container.querySelector('button')!.getAttribute('title');
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  vi.useFakeTimers();
  writeText = vi.fn().mockResolvedValue(undefined);
  // jsdom implements no clipboard, so the button is given one to talk to.
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText },
    configurable: true,
  });
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  vi.useRealTimers();
});

describe('CopyCode', () => {
  it('confirms a copy and then lets the confirmation go', async () => {
    button();
    expect(title()).toBe('Copy code');

    await click();
    expect(writeText).toHaveBeenCalledWith('graph LR');
    expect(title()).toBe('Copied');

    await act(async () => {
      vi.advanceTimersByTime(1500);
    });
    expect(title()).toBe('Copy code');
  });

  it('keeps the confirmation when the button is clicked again inside the window', async () => {
    // Two clicks left two timers running, so the first cleared the confirmation
    // the second had just set — the tick went dark almost as soon as it appeared.
    button();
    await click();

    await act(async () => {
      vi.advanceTimersByTime(1400);
    });
    await click();
    expect(title()).toBe('Copied');

    // Past the first click's window but inside the second's.
    await act(async () => {
      vi.advanceTimersByTime(200);
    });
    expect(title()).toBe('Copied');

    await act(async () => {
      vi.advanceTimersByTime(1400);
    });
    expect(title()).toBe('Copy code');
  });

  it('does not claim success when the write is refused', async () => {
    // An unfocused WebView can refuse the write. Reporting a copy that never
    // happened is worse than saying nothing.
    writeText.mockRejectedValue(new Error('denied'));
    button();

    await click();

    expect(title()).toBe('Copy code');
  });

  it('does not outlive the button', async () => {
    // A confirmation timer left running fires against a component that is gone.
    button();
    await click();
    expect(title()).toBe('Copied');

    act(() => root.unmount());
    container.remove();

    // Nothing to assert but that the clock can run out without complaint.
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    expect(true).toBe(true);
  });
});