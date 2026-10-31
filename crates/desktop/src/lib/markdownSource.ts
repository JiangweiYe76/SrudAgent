// Carries markdown source offsets into the rendered DOM.
//
// The message menu and Ctrl+C copy what the reader selected. A selection is a DOM
// range over rendered content — KaTeX output rather than the TeX the model wrote,
// and prose with the emphasis markers gone — so the rendered nodes carry the
// offsets they came from. `data-md-start` and `data-md-end` are offsets into the
// message's markdown; `data-md-text` marks a span around one text node, where a
// character offset maps directly; `data-md-math` marks a formula's wrapper, whose
// whole source is selected at once.
//
// The plugin runs before `rehype-katex` and `rehype-highlight`: both replace the
// nodes they format, and the replacements carry no source position. Math is
// wrapped first so the wrapper, with the range of its delimiters, survives.

import type { HastNode } from '@/lib/rehypeCodeLines';
import { isTextEntry } from '@/lib/webview';

/** The attributes this plugin adds, and the selection reader looks for. */
export const SOURCE_START = 'data-md-start';
export const SOURCE_END = 'data-md-end';
/** Marks a span wrapping one text node, so an offset inside it maps directly. */
export const SOURCE_TEXT = 'data-md-text';
/** Marks the wrapper around a rendered formula, whose whole source is selected. */
export const SOURCE_MATH = 'data-md-math';

interface SourceNode extends Omit<HastNode, 'properties' | 'children'> {
  properties?: Record<string, unknown>;
  children?: SourceNode[];
  position?: {
    start?: { offset?: number };
    end?: { offset?: number };
  };
}

/** The math classes `remark-math` and `rehype-katex` recognise. */
const MATH_CLASSES = ['math-inline', 'math-display', 'language-math'];

function classesOf(node: SourceNode): string[] {
  const className = node.properties?.className;
  return Array.isArray(className) ? className.map(String) : [];
}

function rangeOf(node: SourceNode): [number, number] | null {
  const start = node.position?.start?.offset;
  const end = node.position?.end?.offset;
  return typeof start === 'number' && typeof end === 'number'
    ? [start, end]
    : null;
}

function isMath(node: SourceNode): boolean {
  return classesOf(node).some((name) => MATH_CLASSES.includes(name));
}

/** Whether a `pre` is the wrapper `remark-rehype` builds for display math. */
function isMathBlock(node: SourceNode): boolean {
  if (node.tagName !== 'pre') return false;
  return (node.children ?? []).some(
    (child) => child.type === 'element' && isMath(child),
  );
}

function withRange(
  properties: Record<string, unknown>,
  range: [number, number],
) {
  properties[SOURCE_START] = String(range[0]);
  properties[SOURCE_END] = String(range[1]);
  return properties;
}

/** A wrapper carrying the source range of the node it contains. */
function wrapper(
  tagName: 'div' | 'span',
  range: [number, number],
  extra: Record<string, unknown>,
  child: SourceNode,
): SourceNode {
  return {
    type: 'element',
    tagName,
    properties: withRange(extra, range),
    children: [child],
  };
}

/**
 * Annotates the tree with source offsets and wraps text nodes that can be
 * mapped.
 *
 * A node without a position is left alone: there is no source text to point at.
 */
export default function rehypeSourceOffsets(): (tree: SourceNode) => void {
  const annotate = (node: SourceNode): void => {
    if (!node.children) return;

    // Raw HTML is rendered as inert text. Wrapping the nodes between its tags
    // would break that run into pieces, so the block is left as it was; a
    // selection in it resolves to the block's range rather than a fragment.
    const inert = node.children.some((child) => child.type === 'raw');

    const next: SourceNode[] = [];

    for (const child of node.children) {
      if (child.type === 'text') {
        // Code and math are handled through their own wrappers: wrapping their
        // text would split highlighted tokens before the highlighters run, and
        // copying a fence or formula takes the whole block rather than a word
        // inside it.
        if (
          inert ||
          (node.type === 'element' &&
            (node.tagName === 'pre' || node.tagName === 'code'))
        ) {
          next.push(child);
          continue;
        }

        const range = rangeOf(child);
        if (!range) {
          next.push(child);
          continue;
        }

        next.push(wrapper('span', range, { [SOURCE_TEXT]: 'true' }, child));
        continue;
      }

      if (child.type !== 'element') {
        next.push(child);
        continue;
      }

      const range = rangeOf(child);
      if (range) withRange((child.properties ??= {}), range);

      const mathBlock = isMathBlock(child);
      const mathInline = child.tagName !== 'pre' && isMath(child);

      if (range && (mathBlock || mathInline)) {
        // `rehype-katex` replaces the element it renders, so the range has to
        // live on a wrapper around it. Block math keeps a block wrapper.
        const block = mathBlock || classesOf(child).includes('math-display');
        next.push(
          wrapper(
            block ? 'div' : 'span',
            range,
            { [SOURCE_MATH]: 'true' },
            child,
          ),
        );
        continue;
      }

      next.push(child);

      // A fence or inline code is one source range; its internals are rendered
      // text the highlighter and line splitter rewrite.
      if (child.tagName === 'pre' || child.tagName === 'code') continue;

      annotate(child);
    }

    node.children = next;
  };

  return (tree: SourceNode): void => {
    annotate(tree);
  };
}

function offsetOf(element: Element, attribute: string): number | null {
  const value = element.getAttribute(attribute);
  if (value === null) return null;
  const offset = Number(value);
  return Number.isFinite(offset) ? offset : null;
}

/** The source range of the element or its nearest annotated ancestor. */
function edgeOf(element: Element | null, edge: 'start' | 'end'): number | null {
  const marked = element?.closest(`[${SOURCE_START}]`);
  if (!marked) return null;
  return offsetOf(marked, edge === 'start' ? SOURCE_START : SOURCE_END);
}

function boundaryOffset(
  node: Node,
  offset: number,
  edge: 'start' | 'end',
  root: Element,
): number | null {
  if (node.nodeType === Node.TEXT_NODE) {
    const text = node as Text;
    const host = text.parentElement?.closest(`[${SOURCE_TEXT}]`);
    if (host) {
      const start = offsetOf(host, SOURCE_START);
      const end = offsetOf(host, SOURCE_END);
      if (start !== null && end !== null) {
        const within = Math.min(Math.max(offset, 0), text.data.length);
        return Math.min(start + within, end);
      }
    }
    return (
      edgeOf(text.parentElement, edge) ??
      rootEdge(root, edge, text.parentElement)
    );
  }

  return (
    edgeOf(node instanceof Element ? node : null, edge) ??
    rootEdge(root, edge, node)
  );
}

/**
 * The first or last source range when a boundary sits on the message wrapper
 * itself, which carries no position of its own.
 */
function rootEdge(
  root: Element,
  edge: 'start' | 'end',
  node: Node | null,
): number | null {
  if (node !== root) return null;

  const units = root.querySelectorAll(`[${SOURCE_START}]`);
  if (units.length === 0) return null;

  const unit = edge === 'start' ? units[0] : units[units.length - 1];
  return offsetOf(unit, edge === 'start' ? SOURCE_START : SOURCE_END);
}

/**
 * The markdown source selected by `selection`, or `null` when the selection
 * does not map back onto `source`.
 *
 * Sorting out the boundaries separately is what keeps a formula whole: a
 * selection inside KaTeX's generated spans starts and ends on text nodes that
 * only share the wrapper around the formula as their nearest annotated ancestor,
 * so both edges resolve to the wrapper's `$$…$$`.
 */
export function sourceOfSelection(
  selection: Selection,
  root: Element,
  source: string,
): string | null {
  if (selection.rangeCount === 0) return null;

  const range = selection.getRangeAt(0);
  if (
    !root.contains(range.startContainer) ||
    !root.contains(range.endContainer)
  ) {
    return null;
  }

  const start = boundaryOffset(
    range.startContainer,
    range.startOffset,
    'start',
    root,
  );
  const end = boundaryOffset(range.endContainer, range.endOffset, 'end', root);
  if (start === null || end === null || start >= end) return null;

  return source.slice(start, end);
}

/** The markdown behind each rendered message, remembered for the copy event. */
const sources = new WeakMap<Element, string>();

/**
 * Remembers the markdown a message was rendered from.
 *
 * The DOM carries offsets, not the source itself. Keeping a whole message in an
 * attribute would duplicate it on every token while streaming, so the WeakMap
 * holds it beside the element instead.
 */
export function rememberMarkdownSource(root: Element, source: string): void {
  sources.set(root, source);
}

/** The markdown selected inside a rendered message, if the selection maps. */
function selectionSource(target: Document): string | null {
  const selection = target.getSelection();
  if (!selection || selection.rangeCount === 0) return null;

  const range = selection.getRangeAt(0);
  const node = range.commonAncestorContainer;
  const element = node instanceof Element ? node : node.parentElement;
  const root = element?.closest('.message-markdown');
  if (!root) return null;

  const source = sources.get(root);
  if (source === undefined) return null;

  return sourceOfSelection(selection, root, source);
}

/** Whether a key press is the copy shortcut, Ctrl+C or Cmd+C. */
function isCopyShortcut(event: KeyboardEvent): boolean {
  if (!(event.ctrlKey || event.metaKey) || event.altKey || event.shiftKey)
    return false;

  // The physical key survives non-Latin layouts, where `key` is not "c".
  return event.code === 'KeyC' || event.key.toLowerCase() === 'c';
}

/**
 * Copies a message selection as the markdown behind it.
 *
 * Ctrl+C is intercepted at the key press and written through the clipboard API,
 * the path the message menu uses. A `copy` event is caught as well, for copies
 * started another way, and writes to its clipboard data when the event carries
 * any. Without either, the browser would copy the rendered selection: KaTeX's
 * output instead of the TeX, and prose with its emphasis markers stripped. Only
 * selections wholly inside a rendered message are touched, so the composer, the
 * sidebar and code-block buttons keep their own copy behaviour.
 *
 * Returns a function that removes the listeners.
 */
export function preserveMarkdownWhenCopying(
  target: Document = document,
): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    if (!isCopyShortcut(event) || isTextEntry(event.target)) return;

    const text = selectionSource(target);
    if (text === null) return;

    const clipboard = target.defaultView?.navigator.clipboard;
    if (!clipboard?.writeText) return;

    event.preventDefault();
    void clipboard.writeText(text).catch(() => {});
  };

  const onCopy = (event: Event) => {
    const text = selectionSource(target);
    if (text === null) return;

    const clipboard = (event as ClipboardEvent).clipboardData;
    if (clipboard) {
      clipboard.setData('text/plain', text);
      event.preventDefault();
      return;
    }

    // A copy event may arrive without clipboard data; the clipboard API still
    // carries the source, so the browser's rendered copy is not used.
    const writer = target.defaultView?.navigator.clipboard;
    if (!writer?.writeText) return;

    event.preventDefault();
    void writer.writeText(text).catch(() => {});
  };

  target.addEventListener('keydown', onKeyDown);
  target.addEventListener('copy', onCopy);
  return () => {
    target.removeEventListener('keydown', onKeyDown);
    target.removeEventListener('copy', onCopy);
  };
}
