// Renders a ```mermaid fence as a diagram instead of as code.
//
// Mermaid parses asynchronously and injects an SVG, so this cannot be done
// during the markdown pass: it needs an effect, a unique id per diagram (mermaid
// writes the result into the element it is given, so two diagrams sharing an id
// overwrite each other), and a failure path — a model that emits a diagram with a
// typo in it should show the broken source, not an empty box.
//
// The library is imported on demand. It is large, and a conversation with no
// diagram in it should not pay for one.
import { useEffect, useId, useState, type ReactNode } from 'react';
import { Code2, Workflow } from 'lucide-react';
import { t } from '@/lib/i18n';
import { cn } from '@/lib/utils';
import { CodeBlockBar } from '@/components/CodeBlockBar';

type State =
  | { status: 'pending' }
  | { status: 'ready'; svg: string }
  | { status: 'failed'; error: string };

/** What the block is showing: the drawn diagram, or the source behind it. */
type View = 'diagram' | 'code';

/**
 * Mermaid's own colours, left at its defaults except for the two that have to
 * follow the app.
 *
 * Its theme is picked once per render rather than watched: re-rendering a diagram
 * on a theme switch costs a full parse, and the alternative — a diagram that stays
 * light in a dark app — is worse than a stale one the user rarely sees.
 */
function configFor(dark: boolean) {
  return {
    startOnLoad: false,
    // Model output is untrusted. `strict` keeps labels as text rather than
    // letting mermaid interpret markup in them.
    securityLevel: 'strict' as const,
    theme: dark ? ('dark' as const) : ('default' as const),
  };
}

/** The error text mermaid throws, trimmed to something a reader can act on. */
function reasonOf(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  // Mermaid prefixes its parse errors with the whole document; the useful part is
  // the last line, which names the construct it choked on.
  const lastLine = message
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean)
    .pop();
  return lastLine ?? t('mermaid.failed');
}

/**
 * A placeholder shown while there is no diagram to draw yet.
 *
 * Centred, and given the height a diagram would roughly take, so the block does
 * not jump the length of the page the moment the real thing arrives.
 */
function Generating() {
  return (
    <div className="mermaid-diagram mermaid-generating" role="status" aria-busy="true">
      <div className="mermaid-skeleton" aria-hidden="true">
        <span className="mermaid-skeleton-bar" />
        <span className="mermaid-skeleton-bar" />
        <span className="mermaid-skeleton-bar" />
      </div>
      <p className="mermaid-pending">{t('mermaid.generating')}</p>
    </div>
  );
}

export function Mermaid({
  chart,
  code,
  streaming = false,
  className,
}: {
  chart: string;
  code?: ReactNode;
  streaming?: boolean;
  className?: string;
}) {
  // `useId` is unique per component instance, which is what mermaid needs: its
  // render target doubles as the id embedded in the SVG.
  const id = useId().replace(/[^a-zA-Z0-9]/g, '');
  const [state, setState] = useState<State>({ status: 'pending' });
  const [view, setView] = useState<View>('diagram');

  useEffect(() => {
    // A chart being written is not a broken chart. CommonMark hands over a fence
    // that is still open as though it were finished, so parsing it here would
    // report a parse error for every chunk of a perfectly good diagram — and
    // would ask mermaid to re-measure the page dozens of times a second, since
    // each render injects a measuring element into the document.
    //
    // Waiting also gets the ordering right for free: the diagram appears once,
    // complete, instead of flickering through states that were never real.
    if (streaming) return;

    let live = true;
    setState({ status: 'pending' });

    void (async () => {
      try {
        const { default: mermaid } = await import('mermaid');
        mermaid.initialize(configFor(document.documentElement.classList.contains('dark')));
        const { svg } = await mermaid.render(`mermaid-${id}`, chart);
        if (live) setState({ status: 'ready', svg });
      } catch (error) {
        // Streaming is over and it still will not parse, so this one really is
        // broken: the source is shown rather than dropped, so the reader can see
        // what was meant and copy it back out.
        if (live) setState({ status: 'failed', error: reasonOf(error) });
      }
    })();

    // The diagram is already gone by the time a late parse resolves; setting
    // state on an unmounted component is wasted work and warns in React.
    return () => {
      live = false;
    };
  }, [chart, id, streaming]);

  // The button says what it will show, not what is on screen, so it reads as
  // "switch to code" rather than "switch to graph" while the graph is up.
  const next: View = view === 'diagram' ? 'code' : 'diagram';

  // A chart that failed to draw has no diagram to show, so the source takes over
  // on its own. Leaving the error up behind a toggle would make the reader click
  // a button just to find out what the chart should have been.
  const failed = state.status === 'failed';
  const shown: View = failed ? 'code' : view;

  // And with no diagram to switch to, the toggle is dropped rather than left in
  // place: it would be offering a view that does not exist, and clicking it
  // would leave the pane exactly as it was.
  const toggle = failed ? null : (
    <button
      className="flex cursor-pointer items-center rounded p-1 text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
      onClick={() => setView(next)}
      title={next === 'code' ? t('mermaid.showCode') : t('mermaid.showDiagram')}
    >
      {next === 'code' ? <Code2 className="h-3.5 w-3.5" /> : <Workflow className="h-3.5 w-3.5" />}
    </button>
  );

  return (
    <div className={cn('code-block', className)}>
      <CodeBlockBar label="mermaid" text={chart}>
        {toggle}
      </CodeBlockBar>
      {shown === 'code' ? (
        <>
          {/* Why the diagram is not there, when there isn't one. The source is
              already on screen below it, so the reader is never left hunting. */}
          {state.status === 'failed' && <p className="mermaid-error">{state.error}</p>}
          {/* The highlighted nodes rather than the chart string: the code view is
              a code view, so it gets the same colours and line numbers as any
              other fence instead of a plain-text wall. */}
          <pre className="mermaid-source">{code ?? <code>{chart}</code>}</pre>
        </>
      ) : state.status === 'ready' ? (
        // Mermaid's SVG carries inline styles and its own colours, so it is
        // inserted as markup rather than parsed by React. The chart is model
        // output, but `securityLevel: 'strict'` above is what makes it inert:
        // mermaid escapes labels rather than rendering them.
        <div className="mermaid-diagram" dangerouslySetInnerHTML={{ __html: state.svg }} />
      ) : (
        // Pending can mean two things: the message is still arriving, or mermaid
        // is a moment behind it. Both are the same thing to a reader — a diagram
        // is on its way — so both get the same centred placeholder.
        <Generating />
      )}
    </div>
  );
}
