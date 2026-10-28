// Promotes a `$$…$$` written on one line to display math.
//
// `remark-math` reads `$$…$$` as inline math unless the delimiters sit on their
// own lines, so the same formula renders centered in block form and as a plain
// inline run when kept on one line. LaTeX gives `$$…$$` display mode either way:
// it is set off from the surrounding paragraph and centered. The plugin restores
// that reading before the markdown is converted to HTML.
//
// A display formula shares its paragraph with any prose around it, so the
// paragraph is split: prose before and after stay paragraphs, and the formula
// becomes a block of its own. `$…$` is untouched and stays inline.

interface MarkdownNode {
  type: string;
  value?: string;
  position?: {
    start?: { offset?: number };
    end?: { offset?: number };
  };
  children?: MarkdownNode[];
  [key: string]: unknown;
}

interface VFileLike {
  value?: unknown;
}

function rangeOf(node: MarkdownNode): [number, number] | null {
  const start = node.position?.start?.offset;
  const end = node.position?.end?.offset;
  return typeof start === 'number' && typeof end === 'number' ? [start, end] : null;
}

/** Whether an inline math node was written with `$$` delimiters. */
function isDisplayMath(node: MarkdownNode, source: string): boolean {
  if (node.type !== 'inlineMath') return false;

  const range = rangeOf(node);
  if (!range) return false;

  return (
    source.slice(range[0], range[0] + 2) === '$$' &&
    source.slice(range[1] - 2, range[1]) === '$$'
  );
}

/**
 * The flow math node `remark-math` would have produced for a `$$` block.
 *
 * The data is the HTML `remark-rehype` reads: `pre` holding a `code` element
 * marked as display math, which `rehype-katex` then renders in display mode.
 */
function blockMath(node: MarkdownNode): MarkdownNode {
  const value = node.value ?? '';

  return {
    type: 'math',
    meta: null,
    value,
    position: node.position,
    data: {
      hName: 'pre',
      hChildren: [
        {
          type: 'element',
          tagName: 'code',
          properties: { className: ['language-math', 'math-display'] },
          children: [{ type: 'text', value }],
        },
      ],
    },
  };
}

function hasContent(children: MarkdownNode[]): boolean {
  return children.some(
    (child) => child.type !== 'text' || (typeof child.value === 'string' && child.value.trim() !== ''),
  );
}

/** Splits a paragraph around the display formulas it contains. */
function splitParagraph(paragraph: MarkdownNode, source: string): MarkdownNode[] {
  const children = paragraph.children ?? [];
  if (!children.some((child) => isDisplayMath(child, source))) {
    return [paragraph];
  }

  const result: MarkdownNode[] = [];
  let inline: MarkdownNode[] = [];

  const flush = () => {
    if (hasContent(inline)) result.push({ type: 'paragraph', children: inline });
    inline = [];
  };

  for (const child of children) {
    if (isDisplayMath(child, source)) {
      flush();
      result.push(blockMath(child));
    } else {
      inline.push(child);
    }
  }
  flush();

  return result;
}

function promote(node: MarkdownNode, source: string): void {
  if (!node.children) return;

  const next: MarkdownNode[] = [];
  for (const child of node.children) {
    promote(child, source);
    if (child.type === 'paragraph') {
      next.push(...splitParagraph(child, source));
    } else {
      next.push(child);
    }
  }

  node.children = next;
}

/**
 * Treats single-line `$$…$$` as display math, the way LaTeX does.
 *
 * Returns a transformer, as `unified` expects from a remark plugin.
 */
export default function remarkDisplayMath(): (tree: MarkdownNode, file: VFileLike) => void {
  return (tree, file) => {
    const source = typeof file.value === 'string' ? file.value : '';
    promote(tree, source);
  };
}
