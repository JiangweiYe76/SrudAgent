// A highlight.js grammar for mermaid, which ships with none.
//
// Without this a `mermaid` fence is left as plain text, so the code view of a
// diagram reads as an unstyled wall even though every other fence in the message
// is coloured.
//
// The grammar is deliberately shallow. Mermaid has around twenty diagram types
// with their own vocabularies, and a half-accurate deep parse would be more
// misleading than a light touch: what a reader needs to see is the diagram type,
// the keywords that change its shape, the edges, and the labels. A keyword
// highlighted slightly too eagerly is a far smaller problem than a chart that
// renders as noise.
import type { LanguageFn } from 'highlight.js';

/**
 * The link forms mermaid accepts, as one pattern.
 *
 * Covers solid (`-->`, `---`), dotted (`-.->`, `-.-`), thick (`==>`, `===`), the
 * bidirectional and circled variants (`<-->`, `o--o`, `x--x`), and the same forms
 * carrying an inline label (`-- text -->`).
 *
 * Two connector characters are required, which is what stops the hyphen in
 * `well-known` from being coloured as an arrow while still matching `---`.
 */
const LINK = /[x<]?[-=.](?:[-=.]+|[a-z0-9_+ ]+[-=.]+)[x>o]?[-=.]*/;

const KEYWORDS = [
  // Diagram declarations.
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
  'requirementDiagram',
  'C4Context',
  // Structure.
  'subgraph',
  'end',
  'direction',
  'title',
  'section',
  'style',
  'linkStyle',
  'classDef',
  'class',
  'click',
  'href',
  // Sequence diagram statements.
  'participant',
  'actor',
  'as',
  'autonumber',
  'activate',
  'deactivate',
  'note',
  'loop',
  'alt',
  'else',
  'opt',
  'par',
  'and',
  'rect',
  'critical',
  'break',
  // Assignment-style keywords, which read better as literals.
  'left',
  'right',
  'of',
  'state',
  'dateFormat',
  'axisFormat',
  'excludes',
  'includes',
];

/** Layout directions, which are a fixed small set and read as literals. */
const LITERALS = ['TB', 'TD', 'BT', 'RL', 'LR'];

const mermaid: LanguageFn = (hljs) => ({
  name: 'Mermaid',
  keywords: { keyword: KEYWORDS, literal: LITERALS },
  contains: [
    // A directive such as `%%{init: {...}}%%` is a whole line, and a plain `%%`
    // runs to the end of one.
    hljs.COMMENT('%%', '$'),
    hljs.QUOTE_STRING_MODE,
    hljs.NUMBER_MODE,
    {
      className: 'operator',
      begin: LINK,
      relevance: 10,
    },
    // Labels in `[square]`, `(round)` and `{rhombus}` brackets, which is most of
    // what a flowchart line is made of. Quoted text inside one is still matched,
    // so `A["quoted"]` keeps its string colour instead of being flattened into
    // the label.
    {
      className: 'title',
      begin: /[([{]/,
      end: /[)\]}]/,
      contains: [hljs.QUOTE_STRING_MODE],
      relevance: 0,
    },
  ],
});

export default mermaid;
