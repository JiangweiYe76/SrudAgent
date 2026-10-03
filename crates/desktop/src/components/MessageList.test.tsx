// @vitest-environment jsdom
//
// Covers the wait between submitting a message and the model's first token
// arriving. That gap renders nothing at all, so it is the easiest state in the app
// to break without noticing: delete the indicator and the only symptom is a
// reader who cannot tell whether the app heard them.
import { describe, it, expect, beforeAll, beforeEach, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { MessageList } from './MessageList';
import type { Step, Turn } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

// jsdom implements no ResizeObserver and no layout, so both are stood in for. The
// observer here keeps its callbacks so a test can fire one by hand, which is the
// only way to exercise the follow-on-late-growth path deterministically.
let resizeCallbacks: (() => void)[] = [];

/** The turns the list was last asked to render, so a test can re-render them. */
let lastTurns: Turn[] = [];

class ManualResizeObserver {
  private readonly callback: () => void;

  constructor(callback: () => void) {
    this.callback = callback;
    resizeCallbacks.push(callback);
  }

  observe(): void {}
  unobserve(): void {}

  disconnect(): void {
    // Dropping the callback is what stops a stale observer from following the list
    // after the component is gone.
    resizeCallbacks = resizeCallbacks.filter((cb) => cb !== this.callback);
  }
}

/** Notifies the list that the content it is watching has changed size. */
function fireResize(): void {
  act(() => {
    for (const callback of [...resizeCallbacks]) callback();
  });
}

/** A step carrying nothing, which an empty chunk can produce. */
function emptyStep(id: string): Step {
  return { id, assistantText: '', toolCalls: [] };
}

function turn(partial: Partial<Turn> & { id: string }): Turn {
  return {
    userInput: 'draw me a flowchart',
    steps: [],
    createdAt: 0,
    ...partial,
  };
}

/** Renders one turn and returns its container. */
function render(turns: Turn[]): HTMLElement {
  lastTurns = turns;
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<MessageList turns={turns} />);
  });
  return container;
}

/**
 * Gives the scroll container a pretend geometry.
 *
 * jsdom reports every element as 0x0 with no scrollable content, so `scrollTop` is
 * always 0 and assigning it is a no-op that proves nothing. Laying the numbers out
 * by hand is what lets the follow behaviour be asserted at all.
 */
function setGeometry(
  list: HTMLElement,
  { scrollHeight, clientHeight }: { scrollHeight: number; clientHeight: number },
): void {
  // The current position carries across, because growing the content does not move
  // the scroll offset in a real browser either. Resetting it here would silently
  // undo the scroll a test had just performed.
  let scrollTop = list.scrollTop;
  Object.defineProperties(list, {
    scrollHeight: { value: scrollHeight, configurable: true },
    clientHeight: { value: clientHeight, configurable: true },
    // Browsers clamp `scrollTop` to the scrollable range; jsdom does not. Without
    // the clamp, assigning the bottom would leave `scrollTop` past the end and every
    // distance calculation would come out negative.
    scrollTop: {
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = Math.max(0, Math.min(value, scrollHeight - clientHeight));
      },
      configurable: true,
    },
  });
}

/** The list's scrolled distance from the bottom, as the list itself computes it. */
function distanceFromBottom(list: HTMLElement): number {
  return list.scrollHeight - list.scrollTop - list.clientHeight;
}

/** The scrollable element the list renders. */
function list(): HTMLElement {
  return container.querySelector<HTMLElement>('.overflow-y-auto')!;
}

/**
 * Re-renders to trigger the follow that a new token would cause.
 *
 * The geometry is faked after the list has mounted, so the follow on mount already
 * ran against jsdom's zeroes. Rendering again is what lets the list respond to the
 * numbers the test has just put in place.
 */
function follow(): void {
  // A fresh array, because the follow keys off the identity of `turns` — the store
  // hands out a new one per token, and re-rendering with the same reference would
  // not count as a new token.
  act(() => {
    root.render(<MessageList turns={[...lastTurns]} />);
  });
}

/** Scrolls the list the way a reader dragging the scrollbar would. */
function scrollTo(list: HTMLElement, scrollTop: number): void {
  act(() => {
    list.scrollTop = scrollTop;
    list.dispatchEvent(new Event('scroll'));
  });
}

/** The three-dot mark, which is the only thing meant to signal a wait. */
function dots(): NodeListOf<Element> {
  return container.querySelectorAll('.animate-thinking-dot');
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  // jsdom implements no layout, so it has no `scrollIntoView` either. The list
  // calls it to follow a growing transcript; that is real browser behaviour, and
  // stubbing it here is cheaper than pretending this suite tests scrolling.
  Element.prototype.scrollIntoView = () => {};
  globalThis.ResizeObserver = ManualResizeObserver as unknown as typeof ResizeObserver;
});

beforeEach(() => {
  resizeCallbacks = [];
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
});

describe('following the bottom while streaming', () => {
  it('lands on the bottom as tokens arrive, without animating', () => {
    // The old code used `scrollIntoView({ behavior: 'smooth' })` on every token.
    // Re-targeting a smooth scroll that often means it never completes: measured in
    // a real browser over a 60-chunk stream, it ended 1320px — the whole height of
    // the content — short of the bottom, having moved nowhere at all.
    render([turn({ id: 't1', steps: [{ id: 's1', assistantText: 'first', toolCalls: [] }] })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });

    act(() => {
      root.render(
        <MessageList
          turns={[
            turn({
              id: 't1',
              steps: [{ id: 's1', assistantText: 'first and more and more', toolCalls: [] }],
            }),
          ]}
        />,
      );
    });

    // Exactly at the bottom: no animation in flight, nothing left to catch up to.
    expect(distanceFromBottom(el)).toBe(0);
    expect(el.scrollTop).toBe(1600);
  });

  it('never leaves the reader above the bottom while they are following', () => {
    render([turn({ id: 't1' })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });

    // Each token grows the transcript; the follow must keep up on every one.
    for (let i = 0; i < 20; i++) {
      setGeometry(el, { scrollHeight: 2000 + i * 30, clientHeight: 400 });
      act(() => {
        root.render(
          <MessageList
            turns={[turn({ id: 't1', steps: [{ id: 's1', assistantText: 'x'.repeat(i), toolCalls: [] }] })]}
          />,
        );
      });
      expect(distanceFromBottom(el)).toBe(0);
    }
  });

  it('follows content that grows after its tokens, such as a diagram', () => {
    // A mermaid diagram renders asynchronously and makes the transcript taller long
    // after the fence that introduced it. Following only on new tokens would leave
    // the reader short of the bottom with nothing to trigger a correction.
    render([turn({ id: 't1', steps: [{ id: 's1', assistantText: '```mermaid', toolCalls: [] }] })]);
    const el = list();
    setGeometry(el, { scrollHeight: 1000, clientHeight: 400 });
    act(() => {
      root.render(
        <MessageList turns={[turn({ id: 't1', steps: [{ id: 's1', assistantText: 'done', toolCalls: [] }] })]} />,
      );
    });

    // The diagram lands and the content is 600px taller.
    setGeometry(el, { scrollHeight: 1600, clientHeight: 400 });
    fireResize();

    expect(distanceFromBottom(el)).toBe(0);
  });

  it('leaves a reader alone once they scroll up to re-read', () => {
    // Dragging them back to the bottom on every token would make a long answer
    // unreadable while it was still arriving.
    render([turn({ id: 't1' })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });
    follow();
    expect(distanceFromBottom(el)).toBe(0);

    scrollTo(el, 200);

    setGeometry(el, { scrollHeight: 2400, clientHeight: 400 });
    act(() => {
      root.render(
        <MessageList turns={[turn({ id: 't1', steps: [{ id: 's1', assistantText: 'more', toolCalls: [] }] })]} />,
      );
    });

    expect(el.scrollTop).toBe(200);
  });

  it('does not drag a scrolled-up reader back when a diagram lands either', () => {
    render([turn({ id: 't1' })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });
    scrollTo(el, 100);

    setGeometry(el, { scrollHeight: 2600, clientHeight: 400 });
    fireResize();

    expect(el.scrollTop).toBe(100);
  });

  it('resumes following once the reader scrolls back down', () => {
    render([turn({ id: 't1' })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });
    scrollTo(el, 0);
    expect(el.scrollTop).toBe(0);

    // Back to the bottom, within the slop that counts as following.
    scrollTo(el, 1570);

    setGeometry(el, { scrollHeight: 2400, clientHeight: 400 });
    act(() => {
      root.render(
        <MessageList turns={[turn({ id: 't1', steps: [{ id: 's1', assistantText: 'more', toolCalls: [] }] })]} />,
      );
    });

    expect(distanceFromBottom(el)).toBe(0);
  });

  it('follows a new turn even after the reader scrolled back', () => {
    render([turn({ id: 't1' })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });
    scrollTo(el, 0);

    act(() => {
      root.render(
        <MessageList
          turns={[
            turn({ id: 't1', endReason: 'completed', endedAt: 1 }),
            turn({ id: 't2' }),
          ]}
        />,
      );
    });

    // Somebody who just sent a message wants to watch the answer arrive.
    expect(distanceFromBottom(el)).toBe(0);
  });

  it('treats a near-enough position as still following', () => {
    // A handful of pixels short of the bottom is not a reader who has scrolled away,
    // and snapping them to the bottom over that gap would be its own small jump.
    render([turn({ id: 't1' })]);
    const el = list();
    setGeometry(el, { scrollHeight: 2000, clientHeight: 400 });
    scrollTo(el, 1600 - 8);

    setGeometry(el, { scrollHeight: 2400, clientHeight: 400 });
    act(() => {
      root.render(
        <MessageList turns={[turn({ id: 't1', steps: [{ id: 's1', assistantText: 'm', toolCalls: [] }] })]} />,
      );
    });

    expect(distanceFromBottom(el)).toBe(0);
  });
});

describe('waiting for the first token', () => {
  it('marks the wait with three dots', () => {
    render([turn({ id: 't1' })]);

    expect(dots()).toHaveLength(3);
  });

  it('reuses the thinking header mark rather than another spinner', () => {
    // One mark for both states: a second indicator would be the same idea drawn
    // twice, and the two would drift apart. The old indicator was a pulsing
    // `fill-accent` circle, which is what these two absences guard against.
    render([turn({ id: 't1' })]);

    expect(dots()).toHaveLength(3);
    expect(container.querySelector('.animate-pulse')).toBeNull();
    expect(container.querySelector('[class*="fill-accent"]')).toBeNull();
  });

  it('names the wait for a screen reader', () => {
    // The dots are `aria-hidden` so that the thinking header can rely on its own
    // visible label; standing alone they would otherwise announce nothing.
    render([turn({ id: 't1' })]);
    expect(container.querySelector('[role="status"]')?.getAttribute('aria-label')).toBe(
      'Waiting for a reply',
    );
  });

  it('stops once anything has arrived', () => {
    // The answer itself is the progress indicator from here on.
    render([
      turn({ id: 't1', steps: [{ id: 's1', assistantText: 'Here you go.', toolCalls: [] }] }),
    ]);

    expect(dots()).toHaveLength(0);
  });

  it('hands the mark over to the thinking header when reasoning arrives first', () => {
    // Reasoning opens a step before any answer text, so this is the other end of
    // the wait. The header animates its own dots, which must mean the standalone
    // one is gone — and that the mark moved rather than simply disappearing.
    render([turn({ id: 't1', steps: [{ id: 's1', thought: 'Let me think', assistantText: '', toolCalls: [] }] })]);

    expect(container.querySelector('[role="status"]')).toBeNull();
    expect(dots()).toHaveLength(3);
  });

  it('stops when a tool call arrives first', () => {
    render([
      turn({
        id: 't1',
        steps: [
          {
            id: 's1',
            assistantText: '',
            toolCalls: [{ id: 'tc1', name: 'read', args: 'a.ts', result: 'ok' }],
          },
        ],
      }),
    ]);

    expect(container.querySelector('[role="status"]')).toBeNull();
  });

  it('holds the wait through a step that arrived empty', () => {
    // A chunk carrying empty text opens a step with nothing in it. Counting steps
    // instead of their contents would drop the indicator and leave the reader
    // staring at a blank gap.
    render([turn({ id: 't1', steps: [emptyStep('s1')] })]);

    expect(dots()).toHaveLength(3);
  });

  it('shows nothing once the turn has ended', () => {
    render([turn({ id: 't1', endReason: 'completed', endedAt: 1 })]);

    expect(dots()).toHaveLength(0);
  });

  it('marks only the turn that is still waiting', () => {
    // Earlier turns are history; their absence of dots means nothing.
    render([
      turn({ id: 't1', endReason: 'completed', endedAt: 1 }),
      turn({ id: 't2', steps: [emptyStep('s2')] }),
    ]);

    expect(container.querySelectorAll('[role="status"]')).toHaveLength(1);
  });
});