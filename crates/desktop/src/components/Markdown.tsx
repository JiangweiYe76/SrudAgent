import { type ReactNode } from 'react';
import ReactMarkdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkMath from 'remark-math';
import rehypeKatex from 'rehype-katex';
import rehypeHighlight from 'rehype-highlight';
import { common } from 'lowlight';
import rehypeCodeLines, { textOf, type HastNode } from '@/lib/rehypeCodeLines';
import { t } from '@/lib/i18n';
import { CodeBlockBar } from '@/components/CodeBlockBar';
import { cn } from '@/lib/utils';
import 'katex/dist/katex.min.css';

/**
 * The language a fence declared, read back off its `language-*` class.
 *
 * `rehype-highlight` adds `hljs` to that class list too, so the list is searched
 * rather than indexed. Returns `undefined` for a fence with no language, which is
 * the caller's cue to leave the label out rather than print an empty one.
 */
function languageOf(node: HastNode | undefined): string | undefined {
  const className = (node?.children?.[0] as { properties?: { className?: unknown } } | undefined)
    ?.properties?.className;
  const classes = Array.isArray(className) ? className.map(String) : [];
  const declared = classes.find((name) => name.startsWith('language-'));
  return declared?.slice('language-'.length).toLowerCase();
}

/**
 * A fenced block: a titled header carrying the language on the left and a copy
 * button on the right, over the code itself.
 *
 * The header is drawn for every block, labelled or not, so a transcript of blocks
 * keeps one shape instead of stepping in and out as languages come and go.
 *
 * React children are not readable as text, so the text to copy comes from the
 * hast node: syntax highlighting has already rewritten it into spans, which is
 * why `textOf` walks the tree rather than reading one child.
 */
function CodeBlock({
  node,
  children,
  className,
}: {
  node?: HastNode;
  children?: ReactNode;
  className?: string;
}) {
  const language = languageOf(node);
  const source = node ? textOf(node) : '';

  return (
    <div className={cn('code-block', className)}>
      <CodeBlockBar label={language ?? t('code.plainText')} text={source} />
      <pre>{children}</pre>
    </div>
  );
}

/**
 * A table inside a scroll container.
 *
 * A wide table would otherwise widen the whole message column and push the
 * answer off screen; the table keeps its natural width and this scrolls instead.
 */
function TableBlock({ children }: { children?: ReactNode }) {
  return (
    <div className="table-scroll">
      <table>{children}</table>
    </div>
  );
}

const components: Components = {
  pre: CodeBlock,
  table: TableBlock,
};

/**
 * Renders a message's markdown.
 *
 * Raw HTML is deliberately not enabled: assistant text is model output, and
 * `rehype-raw` would turn it into live DOM. Syntax highlighting is
 * `rehype-highlight` rather than Shiki because it runs synchronously — a
 * half-streamed fence would flash unstyled while an async highlighter resolves.
 */
export function Markdown({ children, className }: { children: string; className?: string }) {
  return (
    <div className={cn('message-markdown', className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkMath]}
        // Order matters: highlighting rewrites tokens into spans, so the lines
        // have to be split afterwards or a break inside a multi-line string
        // would drag half a token onto the next line.
        //
        // `detect` stays off: guessing a language from the code's contents
        // mislabels prose and costs time on every re-render.
        rehypePlugins={[
          rehypeKatex,
          [rehypeHighlight, { detect: false, languages: { ...common } }],
          [rehypeCodeLines, {}],
        ]}
        components={components}
      >
        {children}
      </ReactMarkdown>
    </div>
  );
}