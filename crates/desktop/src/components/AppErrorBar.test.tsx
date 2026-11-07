// @vitest-environment jsdom
//
// The bar for a failure that belongs to the app: what went wrong, and the two
// things a reader can do about it.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { AppErrorBar } from './AppErrorBar';

let container: HTMLDivElement;
let root: Root;

function render(node: React.ReactElement): HTMLElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(node);
  });
  return container;
}

beforeAll(() => {
  (
    globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }
  ).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
});

const buttons = (shown: HTMLElement) =>
  [...shown.querySelectorAll('button')] as HTMLButtonElement[];

describe('the app failure bar', () => {
  it('says what failed, in full on hover', () => {
    const shown = render(
      <AppErrorBar
        message="cannot load the session: no such file"
        onRetry={() => {}}
        onDismiss={() => {}}
      />,
    );

    const text = shown.querySelector('span[title]');
    expect(text?.textContent).toBe('cannot load the session: no such file');
    expect(text?.getAttribute('title')).toBe(
      'cannot load the session: no such file',
    );
  });

  it('retries the start it is reporting', () => {
    let retried = 0;
    const shown = render(
      <AppErrorBar
        message="boom"
        onRetry={() => retried++}
        onDismiss={() => {}}
      />,
    );

    const retry = buttons(shown).find((b) => b.textContent === 'Retry')!;
    act(() => retry.click());

    expect(retried).toBe(1);
  });

  it('can be put away', () => {
    let dismissed = 0;
    const shown = render(
      <AppErrorBar
        message="boom"
        onRetry={() => {}}
        onDismiss={() => dismissed++}
      />,
    );

    const close = buttons(shown).find(
      (b) => b.getAttribute('title') === 'Close',
    )!;
    act(() => close.click());

    expect(dismissed).toBe(1);
  });
});
