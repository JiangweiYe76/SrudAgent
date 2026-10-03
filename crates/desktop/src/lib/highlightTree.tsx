// A highlighted syntax tree, as React elements.
//
// `rehype-highlight` gets this for free inside the markdown renderer, where
// unified walks the tree and React renders the result. A tool call's payload is
// highlighted on its own — it is JSON the agent returned, not markdown the model
// wrote — so nothing walks it and the tree has to be walked here.
//
// Only the shapes lowlight emits are handled. It produces text nodes and spans
// carrying highlight classes and nothing else, so a node outside that set is
// dropped rather than guessed at: rendering an unknown tag would mean putting an
// element name this file has no reason to know into the DOM.
import { Fragment, type ReactNode } from 'react';
import type { HastNode } from './rehypeCodeLines';

/** The highlight classes on a node, joined for a `className`. */
function classNameOf(node: HastNode): string | undefined {
  const className = node.properties?.className;
  const classes = Array.isArray(className) ? className.map(String) : [];
  return classes.length > 0 ? classes.join(' ') : undefined;
}

/**
 * Renders one node and its descendants.
 *
 * Keys come from the position in the tree rather than from the node, which has
 * none: the same text recurs throughout a payload (`","`, a repeated key), and a
 * key derived from it would collide.
 */
function renderNode(node: HastNode, key: number): ReactNode {
  if (node.type === 'text') return node.value;
  if (node.type !== 'element') return null;

  // `map` handing its index to `key` is the point, not an accident: a callback's
  // second parameter is the position, which is the only stable key a payload
  // offers. See the note in `rehypeCodeLines` about that argument meaning
  // something else there.
  return (
    <span key={key} className={classNameOf(node)}>
      {node.children?.map(renderNode)}
    </span>
  );
}

/**
 * Renders a highlighted tree as elements.
 *
 * The tree is walked through its root, so a caller passes the whole thing and
 * does not have to know that its children are what carry the tokens.
 *
 * A fragment rather than a wrapper element: this is spliced into an existing
 * `pre`, and a block element inside one is where the line boxes stop lining up.
 */
export function renderHighlighted(tree: HastNode): ReactNode {
  const children = tree.children;
  if (!children || children.length === 0) return null;
  return <Fragment>{children.map(renderNode)}</Fragment>;
}