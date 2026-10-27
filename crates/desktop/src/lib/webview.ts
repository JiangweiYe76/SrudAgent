/**
 * Suppresses the webview's built-in context menu.
 *
 * The menu is the webview's, not this app's: in a development build it offers
 * "Inspect Element" and opens the page's markup over a conversation, and in every
 * build it lists browser actions that mean nothing in a desktop client.
 *
 * Text-entry fields are left alone. Their menu carries cut, copy and paste, which
 * is the only way to reach those actions from the pointer.
 *
 * Returns a function that removes the listener.
 */
export function suppressWebviewContextMenu(target: Document = document): () => void {
  const listener = (event: MouseEvent) => {
    if (isTextEntry(event.target)) return;
    event.preventDefault();
  };

  target.addEventListener('contextmenu', listener);
  return () => target.removeEventListener('contextmenu', listener);
}

/**
 * Stops Ctrl+A from selecting the whole window.
 *
 * The window is not one block of text: select-all drags the sidebar, the header
 * and every message into a single selection, which is not what a reader pressing
 * the shortcut over a conversation wants. Text fields keep it, because selecting
 * a draft before replacing it is exactly what the shortcut is for.
 *
 * Returns a function that removes the listener.
 */
export function suppressSelectAll(target: Document = document): () => void {
  const listener = (event: KeyboardEvent) => {
    if (!isSelectAll(event) || isTextEntry(event.target)) return;
    event.preventDefault();
  };

  target.addEventListener('keydown', listener);
  return () => target.removeEventListener('keydown', listener);
}

/** Whether a key press is the select-all shortcut, Ctrl+A or Cmd+A. */
function isSelectAll(event: KeyboardEvent): boolean {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey) return false;

  // The physical key survives non-Latin layouts, where `key` is not "a".
  return event.code === 'KeyA' || event.key.toLowerCase() === 'a';
}

/** Whether the target is a field the reader types into. */
function isTextEntry(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;

  // Checked both ways: `isContentEditable` is not implemented everywhere, and the
  // attribute also identifies the editable ancestor of a clicked child.
  const editable = target.closest('[contenteditable]');
  const inEditable = editable !== null && editable.getAttribute('contenteditable') !== 'false';

  return (
    target.isContentEditable ||
    inEditable ||
    target.tagName === 'INPUT' ||
    target.tagName === 'TEXTAREA'
  );
}
