// @vitest-environment jsdom
//
// Covers what an opened tool call shows.
//
// The tool call panel is the one place the transcript shows JSON the agent
// produced rather than text the model wrote, and it is the only part that has to
// be opened to be read at all — so a block that renders plain text is invisible
// here and obvious everywhere else. These assert the rendered DOM, not the
// helper behind it: `readJson` has its own tests, and a component wired to the
// wrong thing would pass those.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { MessageList } from './MessageList';
import type { ToolCall, Turn } from '@/lib/types';

let container: HTMLDivElement;
let root: Root;

// jsdom implements no ResizeObserver and no layout. Nothing here is about growth,
// so the callbacks are simply dropped rather than kept to be fired by hand.
class NoopResizeObserver {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

/** Renders one turn holding one tool call, and returns its container. */
function render(toolCall: ToolCall): HTMLElement {
  const turn: Turn = {
    id: 't1',
    userInput: 'run it',
    createdAt: 0,
    endReason: 'completed',
    steps: [{ id: 'st1', assistantText: '', toolCalls: [toolCall] }],
  };
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(<MessageList reopening={false} turns={[turn]} />);
  });
  return container;
}

/**
 * Opens the tool call.
 *
 * The trigger is the button, not the first `[data-state]` element: Radix puts the
 * state on both the trigger button and its wrapper, so reaching for the wrapper
 * would click something that does nothing and the panel would stay shut.
 *
 * `CollapsibleContent` is unmounted while closed, so nothing inside the call
 * exists in the DOM until this runs. Asserting without opening it first would read
 * an empty element and pass.
 */
function openToolCall(): void {
  const trigger = container.querySelector<HTMLButtonElement>('button[data-state="closed"]')!;
  act(() => {
    trigger.click();
  });
}

/** The highlight classes rendered inside the opened panel. */
function highlightedClasses(): string[] {
  return [...container.querySelectorAll('[class*="hljs-"]')].flatMap((node) =>
    [...node.classList].filter((name) => name.startsWith('hljs-')),
  );
}

/**
 * The payload rendered under one of the two labels.
 *
 * Located by label rather than by position in the panel. The arguments come first,
 * so the first `pre` is always theirs — a test asking about the result and reaching
 * for it would read the arguments instead and pass on the wrong block.
 *
 * A payload that is not JSON renders as a `span` rather than a `pre`, so both are
 * read: this is what tells a coloured block from a plain-text one.
 */
function payloadUnder(label: 'Input' | 'Output'): { text: string; coloured: boolean } {
  const block = [...container.querySelectorAll('div')].find(
    (node) => node.firstElementChild?.textContent === label,
  )!;
  const body = block.querySelector('pre, span.font-mono')!;
  return { text: body.textContent ?? '', coloured: body.querySelector('[class*="hljs-"]') !== null };
}

beforeEach(() => {
  // Without this, `act` refuses to run and the click below updates state without
  // rendering, so every assertion would read the panel as it was before the click.
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  globalThis.ResizeObserver ??= NoopResizeObserver as unknown as typeof ResizeObserver;
  container = document.createElement('div');
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

describe('an opened tool call', () => {
  it('colours the arguments it was given', () => {
    render({ id: 'tc1', name: 'bash', args: '{"command":"ls -la"}' });
    openToolCall();

    expect(highlightedClasses()).toContain('hljs-attr');
    expect(highlightedClasses()).toContain('hljs-string');
  });

  it('colours the result it got back', () => {
    render({
      id: 'tc1',
      name: 'bash',
      args: '{"command":"ls -la"}',
      result: '{"stdout":"total 8\\n","exit_code":0}',
    });
    openToolCall();

    // Both sides carry an `hljs-attr` key, so the argument alone would satisfy
    // the assertion above. The number is only in the result.
    expect(highlightedClasses()).toContain('hljs-number');
  });

  it('shows both sides under a label each', () => {
    render({ id: 'tc1', name: 'bash', args: '{"command":"ls"}', result: '{"exit_code":0}' });
    openToolCall();

    const labels = [...container.querySelectorAll('span')].map((s) => s.textContent);
    expect(labels).toContain('Input');
    expect(labels).toContain('Output');
  });

  it('puts the arguments on their own lines', () => {
    render({ id: 'tc1', name: 'bash', args: '{"command":"ls"}' });
    openToolCall();

    // The payload arrives as one line. A block that stayed on one line would be
    // no easier to read than the plain text it replaced.
    expect(payloadUnder('Input').text).toBe('{\n  "command": "ls"\n}');
  });

  it('leaves a plain-text result uncoloured', () => {
    // A tool may answer with a sentence rather than a payload. Colouring it would
    // mark up its punctuation as structure that is not there.
    render({
      id: 'tc1',
      name: 'bash',
      args: '{"command":"rm -rf x"}',
      result: 'rm is not available through this tool.',
    });
    openToolCall();

    expect(container.textContent).toContain('rm is not available through this tool.');
    // Scoped to the result block: the arguments are valid JSON and are expected to
    // be coloured, so a panel-wide check would only ever see their punctuation.
    // The sentence has a full stop in it, and marking that up as JSON punctuation
    // is what this rules out.
    const refusal = payloadUnder('Output');
    expect(refusal.text).toBe('rm is not available through this tool.');
    expect(refusal.coloured).toBe(false);
  });

  it('says so when there is no result yet', () => {
    render({ id: 'tc1', name: 'bash', args: '{"command":"ls"}' });
    openToolCall();

    expect(container.textContent).toContain('(no result)');
  });

  it('keeps a payload carrying a markdown fence intact', () => {
    // The read tool pointed at a markdown file returns one. Rendering this
    // through the markdown pipeline would end the block at the fence and show
    // the rest of the JSON as prose.
    const result = JSON.stringify({ content: '# Title\n\n```js\nconst x = 1;\n```\n' });
    render({ id: 'tc1', name: 'read', args: '{"path":"/tmp/a.md"}', result });
    openToolCall();

    expect(payloadUnder('Output').text).toContain('```js');
    expect(highlightedClasses()).toContain('hljs-string');
  });

  it('leaves out the input side of a tool that takes no arguments', () => {
    // A label over an empty space reads as a payload that failed to load, which
    // is a different thing from a tool that has no arguments to show.
    render({ id: 'tc1', name: 'now', args: '', result: '{"ok":true}' });
    openToolCall();

    expect(container.textContent).not.toContain('Input');
    expect(payloadUnder('Output').text).toBe('{\n  "ok": true\n}');
  });

  it('offers no copy button for a side that has nothing to copy', () => {
    render({ id: 'tc1', name: 'bash', args: '{"command":"ls"}' });
    openToolCall();

    // Counted by the copy button's own title, not by `button[title]`: that would
    // also count the collapsible trigger and the message copy button, and pass or
    // fail for reasons that have nothing to do with the payload.
    const copyButtons = container.querySelectorAll('button[title="Copy code"]');
    expect(copyButtons).toHaveLength(1);
  });
});