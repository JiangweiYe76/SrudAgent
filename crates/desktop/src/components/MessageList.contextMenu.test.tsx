// @vitest-environment jsdom
//
// Only what the model said, and what the reader said, belongs to the message
// menu. These tests pin that boundary and what copying does: take the message or
// the selected part of it, through the menu and through Ctrl+C, as the markdown
// the model wrote rather than rendered output.
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { MessageList } from './MessageList';
import { preserveMarkdownWhenCopying } from '@/lib/markdownSource';
import type { Turn } from '@/lib/types';

const mocks = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock('sonner', () => ({ toast: mocks }));

let container: HTMLDivElement;
let root: Root;

class NoopResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** One finished turn with everything a turn can carry. */
function turn(): Turn {
  return {
    id: 't1',
    userInput: 'what is in this file?',
    steps: [
      {
        id: 's1',
        thought: 'I should read it first.',
        assistantText: 'The file holds a greeting.',
        toolCalls: [{ id: 'tc1', name: 'read', args: '{"path":"a.ts"}', pending: false, result: 'ok' }],
      },
    ],
    createdAt: 0,
    endReason: 'completed',
    endedAt: 1,
  };
}

function render(turns: Turn[]): void {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<MessageList reopening={false} turns={turns} />);
  });
}

/** Dispatches a right-click and hands the event back for `defaultPrevented`. */
function rightClick(target: Element, client = { x: 40, y: 60 }): MouseEvent {
  const event = new MouseEvent('contextmenu', {
    bubbles: true,
    cancelable: true,
    clientX: client.x,
    clientY: client.y,
  });
  act(() => {
    target.dispatchEvent(event);
  });
  return event;
}

function menu(): HTMLElement | null {
  return document.querySelector<HTMLElement>('[role="menu"]');
}

function copyItem(): HTMLElement {
  return document.querySelector<HTMLElement>('[role="menuitem"]')!;
}

/** Clicks Copy and lets the clipboard promise settle. */
async function clickCopy(): Promise<void> {
  await act(async () => {
    copyItem().click();
    await Promise.resolve();
    await Promise.resolve();
  });
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  globalThis.ResizeObserver = NoopResizeObserver as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText: vi.fn().mockResolvedValue(undefined) },
    configurable: true,
  });
  mocks.success.mockClear();
  mocks.error.mockClear();
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  window.getSelection()?.removeAllRanges();
});

describe('the message context menu', () => {
  it('opens on a user message and cancels the webview menu', () => {
    render([turn()]);
    const bubble = container.querySelector<HTMLElement>('.bg-user-bubble')!;

    const event = rightClick(bubble);

    expect(event.defaultPrevented).toBe(true);
    expect(menu()).not.toBeNull();
    expect(menu()!.textContent).toContain('Copy');
  });

  it('copies the model message it was opened on', async () => {
    render([turn()]);
    rightClick(container.querySelector('.message-markdown')!);

    await clickCopy();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('The file holds a greeting.');
    expect(mocks.success).toHaveBeenCalledWith('Copied');
    expect(menu()).toBeNull();
  });

  it('copies the selection when the reader had picked one out', async () => {
    render([turn()]);
    const paragraph = container.querySelector('.message-markdown p')!;
    const range = document.createRange();
    range.selectNodeContents(paragraph);
    window.getSelection()!.addRange(range);

    rightClick(paragraph);
    await clickCopy();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('The file holds a greeting.');
  });

  it('copies the selected phrase rather than the whole message', async () => {
    render([turn()]);
    const text = container.querySelector('.message-markdown p')!.textContent!;
    const from = text.indexOf('greeting');
    const range = document.createRange();
    range.setStart(container.querySelector('[data-md-text]')!.firstChild!, from);
    range.setEnd(container.querySelector('[data-md-text]')!.firstChild!, from + 'greeting'.length);
    window.getSelection()!.addRange(range);

    rightClick(container.querySelector('.message-markdown p')!);
    await clickCopy();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('greeting');
  });

  it('copies display math as the source the model wrote', async () => {
    // The rendered formula is KaTeX's markup; a reader quoting it wants the TeX,
    // delimiters included, not the visual output.
    const formula: Turn = {
      ...turn(),
      steps: [{ id: 's1', assistantText: '$$\nE = mc^2\n$$', toolCalls: [] }],
    };
    render([formula]);
    const math = container.querySelector('[data-md-math]')!;
    const range = document.createRange();
    range.selectNodeContents(math);
    window.getSelection()!.addRange(range);

    rightClick(math);
    await clickCopy();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('$$\nE = mc^2\n$$');
  });

  it('copies inline math with its delimiters', async () => {
    const formula: Turn = {
      ...turn(),
      steps: [{ id: 's1', assistantText: 'Use $E = mc^2$ here.', toolCalls: [] }],
    };
    render([formula]);
    const math = container.querySelector('[data-md-math]')!;
    // A real drag lands on the text KaTeX generated, not on the wrapper, so only
    // the formula's own source range can carry the delimiters back out.
    const inner = document.createTreeWalker(math, NodeFilter.SHOW_TEXT).nextNode()!;
    const range = document.createRange();
    range.setStart(inner, 0);
    range.setEnd(inner, 1);
    window.getSelection()!.addRange(range);

    rightClick(math);
    await clickCopy();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('$E = mc^2$');
  });

  it('copies a one-line display formula with its delimiters', async () => {
    const formula: Turn = {
      ...turn(),
      steps: [{ id: 's1', assistantText: '$$E = mc^2$$', toolCalls: [] }],
    };
    render([formula]);
    const math = container.querySelector('[data-md-math]')!;
    const range = document.createRange();
    range.selectNodeContents(math);
    window.getSelection()!.addRange(range);

    rightClick(math);
    await clickCopy();

    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('$$E = mc^2$$');
  });

  it('gives the copy event the markdown source of a selected formula', () => {
    const restoreCopy = preserveMarkdownWhenCopying();
    try {
      const formula: Turn = {
        ...turn(),
        steps: [{ id: 's1', assistantText: '$$\nE = mc^2\n$$', toolCalls: [] }],
      };
      render([formula]);
      const math = container.querySelector('[data-md-math]')!;
      const range = document.createRange();
      range.selectNodeContents(math);
      window.getSelection()!.addRange(range);

      const event = new Event('copy', { bubbles: true, cancelable: true });
      const setData = vi.fn();
      Object.defineProperty(event, 'clipboardData', { value: { setData } });
      act(() => {
        document.dispatchEvent(event);
      });

      expect(setData).toHaveBeenCalledWith('text/plain', '$$\nE = mc^2\n$$');
      expect(event.defaultPrevented).toBe(true);
    } finally {
      restoreCopy();
    }
  });

  it('writes the markdown source on Ctrl+C itself', async () => {
    const restoreCopy = preserveMarkdownWhenCopying();
    try {
      const formula: Turn = {
        ...turn(),
        steps: [{ id: 's1', assistantText: '$$\nE = mc^2\n$$', toolCalls: [] }],
      };
      render([formula]);
      const math = container.querySelector('[data-md-math]')!;
      const range = document.createRange();
      range.selectNodeContents(math);
      window.getSelection()!.addRange(range);

      const event = new KeyboardEvent('keydown', {
        key: 'c',
        code: 'KeyC',
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
      });
      await act(async () => {
        math.dispatchEvent(event);
        await Promise.resolve();
      });

      expect(event.defaultPrevented).toBe(true);
      expect(navigator.clipboard.writeText).toHaveBeenCalledWith('$$\nE = mc^2\n$$');
    } finally {
      restoreCopy();
    }
  });

  it('leaves reasoning outside the menu', () => {
    // Thinking is the model working things out, not the answer a reader would
    // quote.
    render([turn()]);
    const thinking = [...container.querySelectorAll('button')].find((button) =>
      button.textContent?.includes('Thinking'),
    )!;

    rightClick(thinking);

    expect(menu()).toBeNull();
  });

  it('leaves tool calls outside the menu', () => {
    render([turn()]);
    const call = [...container.querySelectorAll('button')].find((button) =>
      button.textContent?.includes('read'),
    )!;

    rightClick(call);

    expect(menu()).toBeNull();
  });

  it('does not paint Copy as hovered before the pointer reaches it', () => {
    // The menu opens where the pointer already is, in the padding above the
    // item. Focusing the item on mount would paint it as hovered before the
    // pointer reached it.
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

    expect(document.activeElement).not.toBe(copyItem());
  });

  it('closes on Escape', () => {
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

    // Radix listens on the document rather than the window.
    act(() => {
      document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });

    expect(menu()).toBeNull();
  });

  it('closes when the pointer goes down anywhere else', async () => {
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

    // Radix arms the outside-pointer listener on the next tick, so that the
    // right-click that opened the menu does not immediately close it.
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
    });

    act(() => {
      document.body.dispatchEvent(new MouseEvent('pointerdown', { bubbles: true }));
    });

    expect(menu()).toBeNull();
  });

  it('closes when the transcript scrolls under it', () => {
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

    act(() => {
      container.querySelector('.overflow-y-auto')!.dispatchEvent(new Event('scroll'));
    });

    expect(menu()).toBeNull();
  });
});
