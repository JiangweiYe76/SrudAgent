// One element per line of code, so a CSS counter can number them.
//
// Runs after syntax highlighting: highlighting wraps tokens in spans, and those
// spans have to be split along with the text, or a line break landing inside a
// multi-line string would drag half a token onto the next line.
//
// Splitting removes the newline characters themselves — each line becomes its own
// element and the break lives in the structure instead of the text. Anything
// reading the code back as text has to account for that; see `textOf` below,
// which the copy button depends on.

/**
 * The slice of a hast node this plugin reads.
 *
 * Typed here rather than imported from `hast`, so a transitive dependency does
 * not become a direct one.
 */
export interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: { className?: unknown };
  children?: HastNode[];
}

/** The class marking a single line of code. */
const LINE_CLASS = 'code-line';

/**
 * Whether a node is a line of code produced by this plugin.
 *
 * The marker is a class rather than a data attribute because that is what
 * survives the trip to the DOM: the counter is a CSS concern, and a class is
 * what the stylesheet already keys off.
 */
export function isLine(node: HastNode): boolean {
  const className = node.properties?.className;
  const classes = Array.isArray(className) ? className.map(String) : [];
  return classes.includes(LINE_CLASS);
}

function line(children: HastNode[]): HastNode {
  return { type: 'element', tagName: 'span', properties: { className: [LINE_CLASS] }, children };
}

/**
 * Groups inline content into lines, breaking wherever a text node holds a
 * newline.
 *
 * An element that spans a line break — a triple-quoted string, say — is split
 * into one copy per line rather than being left to cross the boundary, so every
 * line ends up a single element and the counter stays one-per-line.
 */
function groupIntoLines(children: HastNode[]): HastNode[][] {
  const lines: HastNode[][] = [[]];

  for (const child of children) {
    if (child.type === 'text' && typeof child.value === 'string') {
      child.value.split('\n').forEach((part, i) => {
        if (i > 0) lines.push([]);
        if (part) lines[lines.length - 1].push({ type: 'text', value: part });
      });
    } else if (child.type === 'element') {
      const [first, ...rest] = groupIntoLines(child.children ?? []);
      // The element is kept, not its contents: unwrapping it here would strip the
      // highlight class off every token and leave the code uncoloured. Only a
      // line break forces a split, and then each piece is re-wrapped in a copy of
      // the original so the class survives.
      const fitsOnOneLine = rest.length === 0;
      if (fitsOnOneLine) {
        lines[lines.length - 1].push(child);
      } else {
        lines[lines.length - 1].push({ ...child, children: first });
        for (const piece of rest) {
          lines.push([]);
          lines[lines.length - 1].push({ ...child, children: piece });
        }
      }
    } else {
      lines[lines.length - 1].push(child);
    }
  }

  return lines;
}

/**
 * Flattens a subtree back to the text it was built from.
 *
 * Line breaks are put back: splitting a fence into one element per line takes the
 * newline characters out of the text and leaves them in the structure, so
 * reading the code back has to reconstruct them. Joining the lines without this
 * copies the whole block as one unbroken line.
 *
 * Kept beside the plugin that creates the lines so the two halves of that
 * arrangement — making lines, reading them back — cannot drift apart.
 */
export function textOf(node: HastNode): string {
  if (typeof node.value === 'string') return node.value;
  const children = node.children ?? [];
  return children
    .map((child, i) => {
      const text = textOf(child);
      // A line ends where the next one begins, so it carries a break unless it
      // is the last thing in the block.
      return isLine(child) && i < children.length - 1 ? `${text}\n` : text;
    })
    .join('');
}

/**
 * Wraps every line of every fenced block in its own element.
 *
 * Driven off `pre` rather than `code`: an unlabelled fence renders a `code`
 * element with no class on it, indistinguishable from inline code, whereas the
 * `pre` around it is unambiguous.
 *
 * This is the transformer itself, not a factory returning one — unified applies
 * a plugin by calling it with the parser's options, and the return value is used
 * as the transformer. Returning a function from a factory-shaped export would
 * install a transformer that returns another function, which unified ignores.
 */
export default function rehypeCodeLines(): (tree: HastNode) => void {
  return (tree: HastNode): void => {
    const walk = (node: HastNode): void => {
      // Only the `pre` branch needs element handling; everything else just gets
      // walked through, including the root, which is not an element itself.
      if (node.type === 'element' && node.tagName === 'pre') {
        const code = node.children?.find((child) => child.type === 'element');
        if (code?.children) {
          const lines = groupIntoLines(code.children);
          // A fence ending in a newline yields a trailing empty line, which
          // would number one line past the code the model actually wrote.
          if (lines.length > 1 && lines[lines.length - 1].length === 0) lines.pop();
          // Wrapped rather than mapped directly: `map` passes the index as the
          // second argument, which would be read here as the line's contents.
          code.children = lines.map((contents) => line(contents));
        }
        return;
      }

      node.children?.forEach(walk);
    };

    walk(tree);
  };
}