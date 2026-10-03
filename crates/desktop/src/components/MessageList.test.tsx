// @vitest-environment jsdom
//
// Covers the wait between submitting a message and the model's first token
// arriving. That gap renders nothing at all, so it is the easiest state in the app
// to break without noticing: delete the indicator and the only symptom is a
// reader who cannot tell whether the app heard them.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { MessageList } from './MessageList';
import type { Step, Turn } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

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
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<MessageList turns={turns} />);
  });
  return container;
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
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
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