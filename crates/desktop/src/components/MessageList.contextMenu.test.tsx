// @vitest-environment jsdom
//
// Only what the model said, and what the reader said, belongs to the message
// menu. These tests pin that boundary and the two things the menu does: copy the
// message, or copy the part of it the reader selected.
import { describe, it, expect, beforeAll, beforeEach, afterEach, vi } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { MessageList } from './MessageList';
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

function copyItem(): HTMLButtonElement {
  return document.querySelector<HTMLButtonElement>('[role="menuitem"]')!;
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
    // item. Focusing the item as the menu mounted made it look hovered when the
    // pointer had not touched it.
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

    expect(document.activeElement).not.toBe(copyItem());
  });

  it('closes on Escape', () => {
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

    act(() => {
      window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape' }));
    });

    expect(menu()).toBeNull();
  });

  it('closes when the pointer goes down anywhere else', () => {
    render([turn()]);
    rightClick(container.querySelector('.bg-user-bubble')!);

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
