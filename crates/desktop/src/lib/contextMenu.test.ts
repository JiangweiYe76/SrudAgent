// @vitest-environment jsdom
//
// The webview's menu is what a right-click showed before this guard existed:
// browser actions and, in development, the page's markup. These tests pin the
// two halves of the rule — the app cancels it, the composer keeps its own.
import { describe, it, expect, afterEach } from 'vitest';
import { suppressWebviewContextMenu } from './contextMenu';

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

function append(tag: string): HTMLElement {
  const element = document.createElement(tag);
  document.body.appendChild(element);
  return element;
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
    const editable = append('div');
    editable.setAttribute('contenteditable', 'true');
    const child = document.createElement('span');
    editable.appendChild(child);

    expect(rightClick(child).defaultPrevented).toBe(false);
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
