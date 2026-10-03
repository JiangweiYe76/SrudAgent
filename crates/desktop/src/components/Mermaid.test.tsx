// @vitest-environment jsdom
//
// Verifies that a ```mermaid fence becomes an actual diagram, not just that it is
// routed to the right component — the other suite can only see the routing,
// because mermaid needs a DOM to render into.
//
// This file opts into the jsdom environment; the rest of the suite renders to
// static markup, which needs no DOM and should not pay for one.
import { describe, it, expect, afterEach, beforeAll, vi } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { createRoot, type Root } from 'react-dom/client';
import { act } from 'react';
import { Markdown } from './Markdown';

let container: HTMLDivElement;
let root: Root;

beforeAll(() => {
  // React refuses to flush effects inside `act` unless the environment says it is
  // being driven deliberately; without this every render logs a warning.
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

/** A diagram the renderer has drawn, as opposed to an icon in the bar. */
function drawnDiagram(): SVGSVGElement | null {
  return container.querySelector<SVGSVGElement>('.mermaid-diagram svg');
}

/**
 * Waits for the diagram to settle.
 *
 * Mermaid's render is a promise chain with nothing to await, so this polls for the
 * outcome — an SVG, or the error path — rather than guessing a delay.
 *
 * Scoped to `.mermaid-diagram` because the bar's own controls are SVGs too, and
 * matching one of those would return before mermaid had done anything.
 */
async function settle(ms = 5000): Promise<void> {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 25));
    });
    // The placeholder going away counts as settled too: under jsdom a valid chart
    // reaches the renderer and then throws on measurement, so there is no SVG to
    // wait for and without this the wait runs to its full timeout.
    if (
      drawnDiagram() ||
      container.querySelector('.mermaid-error') ||
      !container.querySelector('.mermaid-generating')
    ) {
      return;
    }
  }
}

/** Clicks a control in the block's bar, as a reader would. */
function click(title: string): void {
  const button = container.querySelector<HTMLButtonElement>(`button[title="${title}"]`);
  expect(button, `no control titled "${title}"`).not.toBeNull();
  act(() => button!.click());
}

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
});

function renderInto(markdown: string, streaming = false): void {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => {
    root.render(
      <Markdown streaming={streaming}>{markdown}</Markdown>,
    );
  });
}

/** Re-renders the same block, as a growing message would. */
function update(markdown: string, streaming = false): void {
  act(() => {
    root.render(
      <Markdown streaming={streaming}>{markdown}</Markdown>,
    );
  });
}

describe('mermaid while streaming', () => {
  // A fence still being written reaches this component as a finished block:
  // CommonMark treats input that ends inside a fence as a complete code block.
  // Reporting a parse error for it would show the reader a red box for every
  // chunk of a diagram that is coming out perfectly well.
  const partial = 'Here you go:\n\n```mermaid\ngraph LR\n  A[Start] --> B\n  B -->';

  it('holds a placeholder rather than reporting the half-written chart broken', async () => {
    renderInto(partial, true);
    await settle(300);

    expect(container.querySelector('.mermaid-error')).toBeNull();
    expect(container.querySelector('.mermaid-generating')).not.toBeNull();
    expect(container.textContent).toContain('Generating diagram');
  });

  it('never calls the renderer on a chart that is still arriving', async () => {
    // Each render injects a measuring element into the document, so parsing per
    // chunk would thrash the page dozens of times a second for a chart that
    // cannot parse yet anyway.
    const mermaid = await import('mermaid');
    const render = vi.spyOn(mermaid.default, 'render').mockResolvedValue({
      svg: '<svg></svg>',
      diagramType: 'flowchart',
    } as unknown as Awaited<ReturnType<typeof mermaid.default.render>>);

    renderInto(partial, true);
    // Grow the message the way a stream would.
    for (const tail of ['C', '{Go}', '```\n\nThat is the flow.']) {
      update(partial + tail, true);
    }
    await settle(300);

    expect(render).not.toHaveBeenCalled();
    render.mockRestore();
  });

  it('draws the diagram once the message is complete', async () => {
    renderInto(partial, true);
    await settle(200);
    expect(container.querySelector('.mermaid-generating')).not.toBeNull();

    // The fence closes and the turn ends: now it is a finished chart.
    update(`${partial} C{Go}\n\`\`\`\n\nThat is the flow.`, false);
    await settle();

    // Under jsdom the drawing itself cannot complete — mermaid measures shapes
    // with `getBBox`, which jsdom does not implement — so what is checked here
    // is that it got as far as trying, rather than sitting on the placeholder.
    expect(container.querySelector('.mermaid-generating')).toBeNull();
  });

  it('still lets a reader watch the source arrive', async () => {
    // The placeholder is the default view, not a lock: the code view is exactly
    // where someone might want to watch a chart being written.
    renderInto(partial, true);
    click('Show code');

    expect(container.querySelector('.mermaid-source')!.textContent).toContain('graph LR');
  });
});

describe('mermaid routing', () => {
  // Static markup, so these describe the first render and nothing else — which is
  // the point: routing is decided during the markdown pass, before any effect has
  // had a chance to run. What the diagram then does needs a DOM and is below.
  const render = (markdown: string) => renderToStaticMarkup(<Markdown>{markdown}</Markdown>);

  it('routes a mermaid fence to the diagram, not the code path', () => {
    const html = render('```mermaid\ngraph LR\n  A[Start] --> B{Condition?}\n```');

    // The switch control exists only on the diagram path.
    expect(html).toContain('Show code');
    expect(html).toContain('mermaid-diagram');
    // It shares the block chrome with code. The source is not in the default view
    // — that is what the switch is for — so there is nothing to highlight or
    // number yet; the code view is where both show up.
    expect(html).toContain('code-block-bar');
    expect(html).not.toContain('hljs');
    expect(html).not.toContain('code-line');
  });

  it('leaves other diagram languages as code', () => {
    // Only mermaid is a diagram; `dot` and friends are still code.
    const html = render('```dot\ndigraph { a -> b }\n```');

    expect(html).toContain('code-block');
    expect(html).not.toContain('mermaid-diagram');
    expect(html).not.toContain('Show code');
  });
});

describe('mermaid', () => {
  // A valid chart cannot be asserted to produce an SVG here: mermaid measures its
  // shapes with `getBBox`, which jsdom does not implement, so rendering throws
  // regardless of the chart. Parsing is the part that does not need geometry, so
  // that is what is checked — the SVG itself needs the real WebView.
  it('accepts a valid chart as a diagram', async () => {
    const { default: mermaid } = await import('mermaid');
    mermaid.initialize({ startOnLoad: false, securityLevel: 'strict' });

    await expect(
      mermaid.parse('graph LR\n  A[Start] --> B{Condition?}\n  B -->|yes| C[Do thing]\n'),
    ).resolves.toBeTruthy();
  });

  it('gets the same titled bar as a code block', () => {
    // One shape for both: a header with a label on the left and controls on the
    // right. A diagram in a box of its own would read as a different kind of thing.
    renderInto('```mermaid\ngraph TD\n  A --> B\n```');

    const block = container.querySelector('.code-block');
    expect(block).not.toBeNull();
    const bar = block!.querySelector('.code-block-bar');
    expect(bar).not.toBeNull();
    expect(bar!.querySelector('.code-block-lang')!.textContent).toBe('mermaid');
  });

  it('numbers nothing until the source is asked for', () => {
    // The diagram is the point; line numbers belong to the source, which is only
    // rendered once a reader switches to it. Asserted here in the default view
    // because that is the state in which there is no `pre` to number — the
    // numbers themselves are covered by the code view below.
    renderInto('```mermaid\ngraph TD\n  A --> B\n```');

    expect(container.querySelector('.mermaid-source')).toBeNull();
    expect(container.querySelector('.code-line')).toBeNull();
  });

  it('switches to the source and back', () => {
    renderInto('```mermaid\ngraph LR\n  A[Start] --> B\n```');

    expect(container.querySelector('.mermaid-source')).toBeNull();

    click('Show code');
    const source = container.querySelector('.mermaid-source');
    expect(source).not.toBeNull();
    expect(source!.textContent).toContain('graph LR');

    click('Show diagram');
    expect(container.querySelector('.mermaid-source')).toBeNull();
  });

  it('keeps the copy button in the bar beside the toggle', () => {
    renderInto('```mermaid\ngraph LR\n  A --> B\n```');

    const buttons = container.querySelectorAll('.code-block-bar button');
    // Switch, then copy. The source is worth having either way.
    expect(buttons).toHaveLength(2);
    expect(buttons[0].getAttribute('title')).toBe('Show code');
    expect(buttons[1].getAttribute('title')).toBe('Copy code');
  });

  it('colours and numbers the code view, like any other fence', () => {
    // The whole point of the custom grammar and of letting the line splitter see
    // mermaid: a reader who switches to the source should not land on a plain
    // unnumbered wall next to fences that are all coloured and numbered.
    renderInto('```mermaid\ngraph LR\n  A[Start] --> B{Condition?}\n```');
    click('Show code');

    // The diagram type, the link and the label are what make a chart readable.
    expect(container.querySelector('.hljs-keyword')).not.toBeNull();
    expect(container.querySelector('.hljs-operator')).not.toBeNull();
    expect(container.querySelector('.hljs-title')).not.toBeNull();
    expect(container.querySelectorAll('.code-line')).toHaveLength(2);
  });

  it('shows the source and the reason when the chart is invalid', async () => {
    // A model that emits a broken diagram must not leave the reader with nothing,
    // and must not make them click to find out what went wrong: the failure
    // switches to the source on its own, with the error above it.
    renderInto('```mermaid\nthis is not a diagram at all !!\n```');
    await settle();

    expect(drawnDiagram()).toBeNull();
    expect(container.querySelector('.mermaid-error')).not.toBeNull();
    expect(container.querySelector('.mermaid-source')!.textContent).toContain(
      'this is not a diagram at all',
    );
  });

  it('drops the toggle once a chart has failed, rather than leaving it dead', async () => {
    // The source is already on screen because the chart would not draw, so a
    // switch-to-code button would be offering the view the reader is already
    // looking at. Clicking it changed nothing at all — only the button's own
    // label moved — which is worse than not having one.
    renderInto('```mermaid\nthis is not a diagram at all !!\n```');
    await settle();
    expect(container.querySelector('.mermaid-error')).not.toBeNull();

    const buttons = container.querySelectorAll('.code-block-bar button');
    // Copy is the only control left; the switch is gone with the diagram.
    expect(buttons).toHaveLength(1);
    expect(buttons[0].getAttribute('title')).toBe('Copy code');
    expect(container.querySelector('.mermaid-source')).not.toBeNull();
  });
});
