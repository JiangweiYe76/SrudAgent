// @vitest-environment jsdom
//
// Both guards cancel a webview default the app does not want: the browser's
// right-click menu and select-all over the transcript. These tests pin the rule
// in both directions — the app cancels, text fields keep their own.
import { describe, it, expect, afterEach } from 'vitest';
import { suppressSelectAll, suppressWebviewContextMenu } from './webview';

let stop: (() => void) | undefined;

afterEach(() => {
  stop?.();
  stop = undefined;
  document.body.innerHTML = '';
});

/** Dispatches a right-click and hands back the event, for `defaultPrevented`. */
function rightClick(target: Element): MouseEvent {
  const event = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
  target.dispatchEvent(event);
  return event;
}

/** Dispatches Ctrl/Cmd+A and hands back the event, for `defaultPrevented`. */
function pressA(target: Element, init: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent('keydown', {
    key: 'a',
    code: 'KeyA',
    bubbles: true,
    cancelable: true,
    ...init,
  });
  target.dispatchEvent(event);
  return event;
}

function append(tag: string): HTMLElement {
  const element = document.createElement(tag);
  document.body.appendChild(element);
  return element;
}

/** A contenteditable region with a child, the way the composer's markup nests. */
function editableWithChild(): HTMLElement {
  const editable = append('div');
  editable.setAttribute('contenteditable', 'true');
  const child = document.createElement('span');
  editable.appendChild(child);
  return child;
}

describe('suppressing the webview context menu', () => {
  it('cancels the browser menu over the transcript', () => {
    stop = suppressWebviewContextMenu();

    expect(rightClick(append('p')).defaultPrevented).toBe(true);
  });

  it('keeps the editing menu in a text field', () => {
    stop = suppressWebviewContextMenu();

    expect(rightClick(append('textarea')).defaultPrevented).toBe(false);
    expect(rightClick(append('input')).defaultPrevented).toBe(false);
  });

  it('keeps the editing menu inside editable content', () => {
    stop = suppressWebviewContextMenu();

    expect(rightClick(editableWithChild()).defaultPrevented).toBe(false);
  });

  it('stops cancelling once the guard is removed', () => {
    stop = suppressWebviewContextMenu();
    const paragraph = append('p');
    expect(rightClick(paragraph).defaultPrevented).toBe(true);

    stop();
    stop = undefined;

    expect(rightClick(paragraph).defaultPrevented).toBe(false);
  });
});

describe('suppressing select-all', () => {
  it('cancels Ctrl+A over the transcript', () => {
    stop = suppressSelectAll();

    expect(pressA(append('p'), { ctrlKey: true }).defaultPrevented).toBe(true);
  });

  it('cancels Cmd+A over the transcript', () => {
    stop = suppressSelectAll();

    expect(pressA(append('p'), { metaKey: true }).defaultPrevented).toBe(true);
  });

  it('keeps select-all in a text field', () => {
    stop = suppressSelectAll();

    expect(pressA(append('textarea'), { ctrlKey: true }).defaultPrevented).toBe(false);
    expect(pressA(append('input'), { metaKey: true }).defaultPrevented).toBe(false);
  });

  it('keeps select-all inside editable content', () => {
    stop = suppressSelectAll();

    expect(pressA(editableWithChild(), { ctrlKey: true }).defaultPrevented).toBe(false);
  });

  it('leaves other key presses alone', () => {
    stop = suppressSelectAll();
    const paragraph = append('p');

    expect(pressA(paragraph).defaultPrevented).toBe(false);
    expect(pressA(paragraph, { ctrlKey: true, shiftKey: true }).defaultPrevented).toBe(false);
  });

  it('stops cancelling once the guard is removed', () => {
    stop = suppressSelectAll();
    const paragraph = append('p');
    expect(pressA(paragraph, { ctrlKey: true }).defaultPrevented).toBe(true);

    stop();
    stop = undefined;

    expect(pressA(paragraph, { ctrlKey: true }).defaultPrevented).toBe(false);
  });
});
