// @vitest-environment jsdom
//
// What a failed turn tells the reader: a mapped headline, the raw pair only on
// request, and the controls available on it.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { TurnErrorNotice } from './TurnErrorNotice';

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

describe('a turn that failed', () => {
  it('names the problem instead of repeating the code', () => {
    const shown = render(
      <TurnErrorNotice
        error={{
          code: -32603,
          message: 'the turn ended with an internal error',
        }}
      />,
    );

    expect(shown.textContent).toContain('The model request failed');
    expect(shown.textContent).not.toContain('RpcFailure');
    expect(shown.textContent).not.toContain('-32603');
  });

  it('keeps the code and message for the details view', () => {
    const shown = render(
      <TurnErrorNotice
        error={{
          code: -32603,
          message: 'the turn ended with an internal error',
        }}
      />,
    );
    const details = [...shown.querySelectorAll('button')].find(
      (button) => button.textContent === 'Details',
    )!;

    act(() => details.click());

    const dialog = document.body.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain('-32603');
    expect(dialog?.textContent).toContain(
      'the turn ended with an internal error',
    );
  });

  it('offers a retry only when the caller can take one', () => {
    let retried = 0;
    const shown = render(
      <TurnErrorNotice error={{ message: 'boom' }} onRetry={() => retried++} />,
    );
    const retry = [...shown.querySelectorAll('button')].find(
      (button) => button.textContent === 'Retry',
    )!;

    act(() => retry.click());

    expect(retried).toBe(1);
  });

  it('reads the cause out of the text when there is no code to read', () => {
    // A reloaded failure comes back with the provider's words and nothing else:
    // the status inside them is what names the problem.
    const shown = render(
      <TurnErrorNotice
        error={{ message: 'model request rejected: 429 Too Many Requests' }}
      />,
    );

    expect(shown.textContent).toContain('Too many requests');
    expect(shown.textContent).not.toContain('429');
  });

  it('still reports a turn that failed before any text was kept', () => {
    // A reloaded failure has the end reason but not the sentence.
    const shown = render(<TurnErrorNotice />);

    expect(shown.textContent).toContain('This turn could not finish');
    expect(
      [...shown.querySelectorAll('button')].some(
        (button) => button.textContent === 'Details',
      ),
    ).toBe(false);
  });
});
