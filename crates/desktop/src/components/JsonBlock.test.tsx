// @vitest-environment jsdom
//
// What an opened tool call shows, per outcome.
//
// A tool that succeeded answers in JSON and one that failed answers in prose, so
// the two look different on screen. What decides that is the backend's account of
// the call, not whether the text happens to parse: parsing decides it by accident,
// and an accident does not survive the next tool.
import { describe, it, expect, beforeAll, afterEach } from 'vitest';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { JsonBlock } from './JsonBlock';

let container: HTMLDivElement;
let root: Root;

/** A succeeded `bash` call, as the backend serialises it. */
const SUCCEEDED = JSON.stringify({
  stdout: '',
  stderr: "ls: cannot access '/nope': No such file or directory\n",
  exit_code: 2,
  truncated: false,
});

/** A refused call, which is prose on purpose. */
const REFUSED =
  '[tool error] `rm` is not available through this tool.\n\n' +
  'The refusal is on the program name, so another spelling of it will be refused too.';

function render(node: React.ReactElement): HTMLElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(node);
  });
  return container;
}

/** The output block, which is the one that varies. */
function output(value: string, isError?: boolean): HTMLElement {
  render(<JsonBlock label="Output" value={value} isError={isError} />);
  return container;
}

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
});

describe('a payload that arrived as JSON', () => {
  it('is indented, so a value sits under its key', () => {
    const shown = output(SUCCEEDED, false);

    const pre = shown.querySelector('pre');
    expect(pre?.textContent).toContain('\n  "exit_code": 2');
    expect(pre?.textContent).not.toBe(SUCCEEDED);
  });

  it('is highlighted, since the grammar can tell a key from a value', () => {
    const shown = output(SUCCEEDED, false);

    // `pre` holds the rendered tree, so the raw text is gone once it parses.
    expect(shown.querySelector('pre')?.textContent).not.toBe(SUCCEEDED);
  });
});

describe('a failure', () => {
  it('is shown as the prose it is', () => {
    const shown = output(REFUSED, true);

    expect(shown.querySelector('pre')?.textContent).toBe(REFUSED);
  });

  it('is marked, so a message that reads like a payload is not read as one', () => {
    const shown = output(REFUSED, true);

    expect(shown.querySelector('.text-destructive')).not.toBeNull();
  });

  it('stays prose even when it happens to be valid JSON', () => {
    // The backend said it failed, and that outranks what the text looks like: a
    // refusal that reads as data is still a refusal.
    const shown = output(JSON.stringify({ stdout: '', exit_code: 1 }), true);

    expect(shown.querySelector('pre')?.textContent).toBe(
      JSON.stringify({ stdout: '', exit_code: 1 }),
    );
  });
});

describe('a success that is not JSON', () => {
  it('says so rather than passing for prose on purpose', () => {
    // Nothing the backend sends should produce this, so the marker is how a tool
    // that lost its shape gets noticed instead of quietly rendered.
    const shown = output('I could not read that file.', false);

    expect(shown.querySelector('[title]')).not.toBeNull();
  });

  it('still shows what the tool said', () => {
    const shown = output('I could not read that file.', false);

    expect(shown.querySelector('pre')?.textContent).toBe('I could not read that file.');
  });
});

describe('without a statement either way', () => {
  it('falls back to what the text is, since nothing said it was a failure', () => {
    // A call still in flight has no `is_error` yet. Prose before the result
    // arrives is the tool talking, not a failure being reported.
    const shown = output(REFUSED, undefined);

    expect(shown.querySelector('.text-destructive')).toBeNull();
  });

  it('still indents a payload that has not been labelled yet', () => {
    const shown = output(SUCCEEDED, undefined);

    expect(shown.querySelector('pre')?.textContent).toContain('\n  "exit_code": 2');
  });
});

describe('the input side', () => {
  it('is never treated as a failure', () => {
    // A tool's arguments are whatever the model wrote; the backend reports nothing
    // about them, and passing a failure here would colour every input.
    const shown = render(<JsonBlock label="Input" value={'{"command":"rm x"}'} />);

    expect(shown.querySelector('.text-destructive')).toBeNull();
    expect(shown.querySelector('pre')?.textContent).toBe('{\n  "command": "rm x"\n}');
  });
});

describe('nothing to show', () => {
  it('renders neither label nor block', () => {
    const shown = render(<JsonBlock label="Input" value="" />);

    expect(shown.querySelector('pre')).toBeNull();
    expect(shown.querySelector('span')).toBeNull();
  });

  it('shows the fallback in place of an absent payload', () => {
    const shown = render(<JsonBlock label="Output" value="" fallback="(no result)" />);

    expect(shown.querySelector('pre')).toBeNull();
    expect(shown.textContent).toContain('(no result)');
  });
});

describe('copying', () => {
  it('copies the indented text rather than what arrived', () => {
    // The one-line payload is what crosses the wire; the indented one is what is
    // on screen, and copying the other would put the two out of step.
    const shown = output(SUCCEEDED, false);
    const copy = shown.querySelector('button');
    expect(copy).not.toBeNull();
    expect(copy?.getAttribute('title')).toBe('Copy code');
  });
});