// @vitest-environment jsdom
//
// The box is controlled by the session's draft: every keystroke is reported
// upward, Enter sends the trimmed text, and clearing is the store's job.
import { describe, it, expect, vi, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { ChatInput } from './ChatInput';
import type { Attachment } from '@/lib/types';

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
  vi.mocked(invoke).mockReset();
});

function render(props: Partial<Parameters<typeof ChatInput>[0]> = {}) {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  const handlers = {
    value: '',
    onValueChange: vi.fn(),
    attachments: [],
    onAttachmentsChange: vi.fn(),
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

function image(id = 'img1'): Attachment {
  return { id, mime: 'image/png', data: 'YWJj' };
}

// jsdom has no clipboard, so the paste event carries the file items directly.
function paste(file: File) {
  const item = { kind: 'file', type: file.type, getAsFile: () => file };
  const event = new Event('paste', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'clipboardData', { value: { items: [item], files: [] } });
  act(() => {
    box().dispatchEvent(event);
  });
  return event;
}

// A clipboard naming a file instead of carrying it, which is what a Linux file
// manager puts there.
function pastePath(path: string) {
  const event = new Event('paste', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'clipboardData', {
    value: {
      items: [],
      files: [],
      getData: (type: string) => (type === 'text/uri-list' ? `file://${path}` : ''),
    },
  });
  act(() => {
    box().dispatchEvent(event);
  });
  return event;
}

// A paste the webview hands over with nothing on it: no bytes, no path, no
// text. A file copied in a Linux file manager can arrive this way.
function pasteEmpty() {
  const event = new Event('paste', { bubbles: true, cancelable: true });
  Object.defineProperty(event, 'clipboardData', {
    value: { items: [], files: [], getData: () => '' },
  });
  act(() => {
    box().dispatchEvent(event);
  });
  return event;
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
    expect(onSend).toHaveBeenCalledWith('ask this', []);
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

  it('turns a pasted image into an attachment', async () => {
    const onAttachmentsChange = vi.fn();
    render({ onAttachmentsChange });
    const event = paste(new File(['abc'], 'shot.png', { type: 'image/png' }));
    await vi.waitFor(() => expect(onAttachmentsChange).toHaveBeenCalledTimes(1));
    const [images] = onAttachmentsChange.mock.calls[0] as [Attachment[]];
    expect(images).toHaveLength(1);
    expect(images[0].mime).toBe('image/png');
    expect(images[0].data).toBe('YWJj');
    // A pasted image is taken over, so the browser does not also paste it as text.
    expect(event.defaultPrevented).toBe(true);
  });

  it('reads an image the clipboard names by path', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
    vi.mocked(invoke).mockResolvedValueOnce({ mime: 'image/png', data: 'YWJj' });
    const onAttachmentsChange = vi.fn();
    render({ onAttachmentsChange });

    const event = pastePath('/home/me/a shot.png');

    await vi.waitFor(() => expect(onAttachmentsChange).toHaveBeenCalledTimes(1));
    // The path would otherwise be pasted into the box as text.
    expect(event.defaultPrevented).toBe(true);
    const [images] = onAttachmentsChange.mock.calls[0] as [Attachment[]];
    expect(images[0]).toMatchObject({ mime: 'image/png', data: 'YWJj' });
    // The URI is decoded before the host is asked for the file.
    expect(invoke).toHaveBeenCalledWith('read_image_file', {
      path: '/home/me/a shot.png',
      limit: 10 * 1024 * 1024,
    });
  });

  it('asks the host for a paste that arrives empty', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
    vi.mocked(invoke).mockResolvedValueOnce({ kind: 'image', mime: 'image/png', data: 'YWJj' });
    const onAttachmentsChange = vi.fn();
    render({ onAttachmentsChange });

    const event = pasteEmpty();

    await vi.waitFor(() => expect(onAttachmentsChange).toHaveBeenCalledTimes(1));
    // The default handling is what would write the copied path into the box.
    expect(event.defaultPrevented).toBe(true);
    const [images] = onAttachmentsChange.mock.calls[0] as [Attachment[]];
    expect(images[0]).toMatchObject({ mime: 'image/png', data: 'YWJj' });
    expect(invoke).toHaveBeenCalledWith('read_clipboard', { limit: 10 * 1024 * 1024 });
  });

  it('writes back text the host read from an empty paste', async () => {
    Object.defineProperty(window, '__TAURI_INTERNALS__', { value: {}, configurable: true });
    vi.mocked(invoke).mockResolvedValueOnce({ kind: 'text', text: 'hello' });
    const onValueChange = vi.fn();
    render({ onValueChange });

    pasteEmpty();

    await vi.waitFor(() => expect(onValueChange).toHaveBeenCalledWith('hello'));
  });

  it('leaves an empty paste alone when there is no host to read it', () => {
    const { onValueChange } = render();
    const event = pasteEmpty();
    expect(event.defaultPrevented).toBe(false);
    expect(invoke).not.toHaveBeenCalled();
    expect(onValueChange).not.toHaveBeenCalled();
  });

  it('drops an image the user no longer wants', () => {
    const kept = image('img1');
    const dropped = image('img2');
    const { onAttachmentsChange } = render({ attachments: [kept, dropped] });
    expect(container.querySelectorAll('[aria-label="Remove image"]')).toHaveLength(2);
    click('[aria-label="Remove image"]');
    expect(onAttachmentsChange).toHaveBeenCalledWith([dropped]);
  });

  it('sends an image with no text at all', () => {
    const attached = image();
    const { onSend } = render({ attachments: [attached] });
    press('Enter');
    expect(onSend).toHaveBeenCalledWith('', [attached]);
  });

  it('holds images back while a turn is in flight', () => {
    const { onSend } = render({ attachments: [image()], busy: true });
    press('Enter');
    expect(onSend).not.toHaveBeenCalled();
    expect(container.querySelector('[aria-label="Send"]')!.hasAttribute('disabled')).toBe(true);
  });

  it('opens an attached image large', () => {
    render({ attachments: [image()] });
    click('[aria-label="Image preview"]');
    expect(document.body.querySelector('img[alt="Image preview"]')).not.toBeNull();
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
