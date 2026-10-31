// @vitest-environment jsdom
//
// The composer's right-click menu replaces the webview's: the same menu the
// transcript uses, with the editing actions a text field needs.
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChatInput } from './ChatInput';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  (globalThis as Record<string, unknown>).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  delete (window as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
  Reflect.deleteProperty(navigator, 'clipboard');
  vi.mocked(invoke).mockReset();
});

function render(value: string) {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  const onValueChange = vi.fn();
  act(() => {
    root.render(
      <ChatInput
        value={value}
        onValueChange={onValueChange}
        attachments={[]}
        onAttachmentsChange={vi.fn()}
        onSend={vi.fn()}
        busy={false}
        onStop={vi.fn()}
      />,
    );
  });
  return onValueChange;
}

function box(): HTMLTextAreaElement {
  return container.querySelector('textarea')!;
}

function clipboard() {
  const writeText = vi.fn().mockResolvedValue(undefined);
  const readText = vi.fn().mockResolvedValue('');
  Object.defineProperty(navigator, 'clipboard', {
    value: { writeText, readText },
    configurable: true,
  });
  return { writeText, readText };
}

/** Right-clicks with a remembered selection, the way the menu sees it. */
function openMenu(start = 0, end = 0): MouseEvent {
  box().focus();
  box().setSelectionRange(start, end);
  const event = new MouseEvent('contextmenu', {
    bubbles: true,
    cancelable: true,
    clientX: 24,
    clientY: 32,
  });
  act(() => {
    box().dispatchEvent(event);
  });
  return event;
}

function item(label: string): HTMLElement {
  const found = Array.from(
    document.querySelectorAll<HTMLElement>('[role="menuitem"]'),
  ).find((button) => button.textContent?.trim() === label);
  if (!found) throw new Error(`no menu item: ${label}`);
  return found;
}

function choose(label: string) {
  act(() => {
    item(label).click();
  });
}

describe('the composer context menu', () => {
  it('replaces the webview menu with the editing actions', () => {
    render('half a thought');

    const event = openMenu();

    expect(event.defaultPrevented).toBe(true);
    const menu = document.querySelector('[role="menu"]');
    expect(menu?.getAttribute('aria-label')).toBe('Editing actions');
    expect(
      Array.from(document.querySelectorAll('[role="menuitem"]')).map((button) =>
        button.textContent?.trim(),
      ),
    ).toEqual(['Cut', 'Copy', 'Paste', 'Select all']);
  });

  it('copies the selected text and closes', async () => {
    render('hello world');
    const { writeText } = clipboard();

    openMenu(0, 5);
    choose('Copy');

    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith('hello'));
    expect(document.querySelector('[role="menu"]')).toBeNull();
  });

  it('cuts the selected text out of the draft', async () => {
    const onValueChange = render('hello world');
    const { writeText } = clipboard();

    openMenu(0, 5);
    choose('Cut');

    await vi.waitFor(() => expect(writeText).toHaveBeenCalledWith('hello'));
    expect(onValueChange).toHaveBeenCalledWith(' world');
  });

  it('pastes text the host reads at the remembered caret', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', {
      value: {},
      configurable: true,
    });
    vi.mocked(invoke).mockResolvedValueOnce({ kind: 'text', text: 'pasted' });

    const onValueChange = render('ac');
    clipboard();

    openMenu(1, 1);
    choose('Paste');

    await vi.waitFor(() =>
      expect(onValueChange).toHaveBeenCalledWith('apastedc'),
    );
    expect(invoke).toHaveBeenCalledWith('read_clipboard', {
      limit: 10 * 1024 * 1024,
    });
  });

  it('selects the whole draft', async () => {
    render('hello');

    openMenu(2, 2);
    choose('Select all');

    // Radix returns focus after the menu has finished closing.
    await vi.waitFor(() => expect(document.activeElement).toBe(box()));
    expect(box().selectionStart).toBe(0);
    expect(box().selectionEnd).toBe(5);
  });

  it('leaves cut and copy disabled when nothing is selected', () => {
    render('hello');

    openMenu(2, 2);

    // Radix marks a disabled item rather than rendering a disabled button.
    expect(item('Cut').getAttribute('data-disabled')).not.toBeNull();
    expect(item('Copy').getAttribute('data-disabled')).not.toBeNull();
    expect(item('Paste').getAttribute('data-disabled')).toBeNull();
  });
});
