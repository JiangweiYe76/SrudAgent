// Exercises the mermaid grammar against real chart syntax.
//
// The grammar is the one thing here that cannot be checked by reading it: the
// link patterns are regexes over punctuation, and the only way to know they match
// the forms mermaid actually accepts is to run them.
import { describe, it, expect } from 'vitest';
import { createLowlight, common } from 'lowlight';
import mermaid from './mermaid';

const lowlight = createLowlight({ ...common, mermaid });

/** The slice of a hast node this file reads. */
interface Node {
  type: string;
  properties?: { className?: unknown };
  children?: Node[];
}

/** The highlight.js classes produced for a chart. */
function classesFor(chart: string): string[] {
  const found: string[] = [];
  const walk = (node: Node): void => {
    const className = node.properties?.className;
    if (Array.isArray(className)) found.push(...className.map(String));
    node.children?.forEach(walk);
  };
  walk(lowlight.highlight('mermaid', chart) as unknown as Node);
  return found.filter((value) => value.startsWith('hljs-'));
}

describe('mermaid grammar', () => {
  it('highlights the diagram type', () => {
    expect(classesFor('graph LR\n  A --> B')).toContain('hljs-keyword');
  });

  it.each([
    ['solid', 'graph LR\n  A --> B'],
    ['open', 'graph LR\n  A --- B'],
    ['thick', 'graph LR\n  A ==> B'],
    ['dotted', 'graph LR\n  A -.-> B'],
    ['bidirectional', 'graph LR\n  A <--> B'],
    ['circles', 'graph LR\n  A o--o B'],
    ['crosses', 'graph LR\n  A x--x B'],
  ])('highlights a %s link', (_name, chart) => {
    // Every one of these is a link the grammar must colour, or the edges of a
    // flowchart come out as indistinguishable dashes.
    expect(classesFor(chart)).toContain('hljs-operator');
  });

  it('highlights a link carrying an inline label', () => {
    // `-- yes -->` is one connector as far as colour goes; splitting it would
    // leave the arrow and its label looking like unrelated text.
    expect(classesFor('graph LR\n  A -- yes --> B')).toContain('hljs-operator');
  });

  it('highlights a link label in pipes', () => {
    expect(classesFor('graph LR\n  A -->|maybe| B')).toContain('hljs-operator');
  });

  it('highlights structure keywords', () => {
    expect(classesFor('graph TD\n  subgraph one\n  A\n  end')).toContain('hljs-keyword');
  });

  it('highlights the layout direction as a literal, not a keyword', () => {
    const classes = classesFor('graph LR\n  A --> B');

    expect(classes).toContain('hljs-literal');
  });

  it('highlights labels in brackets', () => {
    expect(classesFor('graph LR\n  A[Start here] --> B')).toContain('hljs-title');
  });

  it('highlights quoted text', () => {
    expect(classesFor('graph LR\n  A["quoted"] --> B')).toContain('hljs-string');
  });

  it('ignores a comment', () => {
    // `%%` runs to the end of the line, so a keyword after it is not a keyword.
    const classes = classesFor('%% graph LR is not a directive here\nflowchart TD\n  A --> B');

    expect(classes).toContain('hljs-comment');
  });

  it('highlights the diagram types it claims to', () => {
    // The keyword list is long enough to rot; a diagram type that quietly stopped
    // matching would render as unstyled text with nothing to notice.
    for (const type of [
      'graph',
      'flowchart',
      'sequenceDiagram',
      'classDiagram',
      'stateDiagram',
      'erDiagram',
      'journey',
      'gantt',
      'pie',
      'quadrantChart',
      'gitGraph',
      'mindmap',
      'timeline',
    ]) {
      expect(classesFor(`${type}\n  A --> B`), type).toContain('hljs-keyword');
    }
  });

  it('does not colour a lone hyphen in a label as an arrow', () => {
    // The reason the link pattern demands two connector characters: without that,
    // every hyphenated word in a label would come out as an arrow. No real link
    // here, or the test would pass on the arrow rather than on the hyphen.
    expect(classesFor('graph LR\n  A[well-known]')).not.toContain('hljs-operator');
  });

  it('leaves an ordinary language alone', () => {
    // Registering mermaid must not disturb the rest of the set: `common` is
    // spread in rather than replaced, and this is the check on that.
    const tree = lowlight.highlight('python', 'def f():\n    return 1\n');

    expect(JSON.stringify(tree)).toContain('hljs-keyword');
  });
});
