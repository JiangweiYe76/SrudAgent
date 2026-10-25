// @vitest-environment jsdom
//
// The box is controlled by the session's draft: every keystroke is reported
// upward, Enter sends the trimmed text, and clearing is the store's job.
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { ChatInput } from './ChatInput';

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function render(props: Partial<Parameters<typeof ChatInput>[0]> = {}) {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  const handlers = {
    value: '',
    onValueChange: vi.fn(),
    onSend: vi.fn(),
    busy: false,
    onStop: vi.fn(),
    ...props,
  };
  act(() => {
    root.render(<ChatInput {...handlers} />);
  });
  return handlers;
}

function box(): HTMLTextAreaElement {
  return container.querySelector('textarea')!;
}

function type(text: string) {
  // Through the native setter: React's value tracker swallows a direct
  // assignment, so the change event would carry the old text.
  const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')!.set!;
  setter.call(box(), text);
  act(() => {
    box().dispatchEvent(new Event('input', { bubbles: true }));
  });
}

function press(key: string, shift = false) {
  act(() => {
    box().dispatchEvent(new KeyboardEvent('keydown', { key, shiftKey: shift, bubbles: true }));
  });
}

function click(selector: string) {
  act(() => {
    container.querySelector<HTMLButtonElement>(selector)!.click();
  });
}

describe('ChatInput', () => {
  it('shows the draft it is given', () => {
    render({ value: 'half a thought' });
    expect(box().value).toBe('half a thought');
  });

  it('reports every keystroke instead of holding the text', () => {
    const { onValueChange } = render();
    type('hello');
    expect(onValueChange).toHaveBeenCalledWith('hello');
    // The box is not the owner, so it does not move on its own.
    expect(box().value).toBe('');
  });

  it('sends the trimmed text on Enter', () => {
    const { onSend, onValueChange } = render({ value: '  ask this  ' });
    press('Enter');
    expect(onSend).toHaveBeenCalledWith('ask this');
    // Clearing is the store's job, done when it takes the turn.
    expect(onValueChange).not.toHaveBeenCalled();
  });

  it('leaves a newline for Shift+Enter', () => {
    const { onSend } = render({ value: 'line one' });
    press('Enter', true);
    expect(onSend).not.toHaveBeenCalled();
  });

  it('sends nothing for an empty or blank draft', () => {
    const { onSend } = render({ value: '   ' });
    press('Enter');
    expect(onSend).not.toHaveBeenCalled();
    expect(container.querySelector('[aria-label="Send"]')!.hasAttribute('disabled')).toBe(true);
  });

  it('sends nothing while a turn is in flight', () => {
    const { onSend } = render({ value: 'too early', busy: true });
    press('Enter');
    expect(onSend).not.toHaveBeenCalled();
    expect(container.querySelector('[aria-label="Send"]')!.hasAttribute('disabled')).toBe(true);
  });

  it('lists waiting messages as separate lines', () => {
    render({
      busy: true,
      followups: ['first waiting', 'second waiting'],
      onSteerFollowup: vi.fn(),
      onRemoveFollowup: vi.fn(),
    });
    const steerButtons = Array.from(container.querySelectorAll('[aria-label="Steer"]'));
    expect(steerButtons).toHaveLength(2);
    expect(container.textContent).toContain('first waiting');
    expect(container.textContent).toContain('second waiting');
    // Each line carries its own pair of actions.
    expect(container.querySelectorAll('[aria-label="Delete"]')).toHaveLength(2);
  });

  it('stops a running turn and nothing else', () => {
    const stopped = render({ value: 'typing anyway', busy: true });
    expect(container.querySelector('[aria-label="Stop"]')!.hasAttribute('disabled')).toBe(false);
    click('[aria-label="Stop"]');
    expect(stopped.onStop).toHaveBeenCalled();

    const idle = render();
    expect(container.querySelector('[aria-label="Stop"]')!.hasAttribute('disabled')).toBe(true);
    click('[aria-label="Stop"]');
    expect(idle.onStop).not.toHaveBeenCalled();
  });
});
