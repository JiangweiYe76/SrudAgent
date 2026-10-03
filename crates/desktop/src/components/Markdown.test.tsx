// Tests the markdown surface against its rendered HTML: the elements a message
// relies on are present, code is highlighted and copyable, and raw HTML stays
// inert because it is model output.
import { describe, it, expect } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { Markdown } from './Markdown';
import rehypeCodeLines, { isLine, textOf } from '@/lib/rehypeCodeLines';

interface TestNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: { className?: unknown };
  children?: TestNode[];
}

function render(markdown: string): string {
  return renderToStaticMarkup(<Markdown>{markdown}</Markdown>);
}

/**
 * Runs the pipeline up to line splitting and returns the `code` element.
 *
 * Going through the plugin directly, rather than scraping rendered HTML, is what
 * lets the copy-button behaviour be asserted: the text of a split block exists
 * only in the tree, since splitting is exactly what removed the line breaks from
 * it.
 */
function splitCode(markdown: string): TestNode {
  // Fenced code arrives as a `pre > code` element holding its own text. That is
  // the shape the plugin is written against, so the fixture is built here rather
  // than by running the whole markdown pipeline.
  const source = markdown.replace(/^```[^\n]*\n?/, '').replace(/\n?```$/, '');
  const tree: TestNode = {
    type: 'root',
    children: [
      {
        type: 'element',
        tagName: 'pre',
        children: [
          {
            type: 'element',
            tagName: 'code',
            properties: { className: ['language-python'] },
            children: [{ type: 'text', value: source }],
          },
        ],
      },
    ],
  };
  rehypeCodeLines()(tree);
  return tree.children![0].children![0];
}

describe('rehypeCodeLines', () => {
  it('wraps each line so a counter has one element per line', () => {
    const code = splitCode('```python\na = 1\nb = 2\nc = 3\n```');

    expect(code.children).toHaveLength(3);
    expect(code.children!.every(isLine)).toBe(true);
  });

  it('puts the line breaks back when the code is read as text', () => {
    const code = splitCode('```python\ndef f(x):\n    return 1\n```');

    expect(textOf(code)).toBe('def f(x):\n    return 1');
  });

  it('leaves a blank line as a line of its own', () => {
    const code = splitCode('```\na\n\nb\n```');

    expect(code.children).toHaveLength(3);
    expect(textOf(code)).toBe('a\n\nb');
  });

  it('does not invent a line for the newline every fence ends with', () => {
    const code = splitCode('```\na\nb\n```');

    expect(code.children).toHaveLength(2);
    expect(textOf(code)).toBe('a\nb');
  });

  it('splits a token that straddles a line break', () => {
    // A multi-line string is one span after highlighting; leaving it whole would
    // put half a token on the next line and number that line wrongly.
    const tree: TestNode = {
      type: 'root',
      children: [
        {
          type: 'element',
          tagName: 'pre',
          children: [
            {
              type: 'element',
              tagName: 'code',
              children: [
                { type: 'text', value: 'x = """' },
                {
                  type: 'element',
                  tagName: 'span',
                  properties: { className: ['hljs-string'] },
                  children: [{ type: 'text', value: 'one\ntwo' }],
                },
              ],
            },
          ],
        },
      ],
    };
    rehypeCodeLines()(tree);
    const code = tree.children![0].children![0];

    // `x = """one` and `two`: the break inside the string is one line boundary.
    expect(code.children).toHaveLength(2);
    expect(textOf(code)).toBe('x = """one\ntwo');
    // The highlight class has to survive the split, or the string loses colour.
    const stringSpan = code.children![1].children![0];
    expect(isLine(stringSpan)).toBe(false);
    expect(stringSpan.properties!.className as string[]).toContain('hljs-string');
  });

  it('leaves a tree with no fenced block alone', () => {
    const tree: TestNode = {
      type: 'root',
      children: [{ type: 'text', value: 'just prose' }],
    };
    const before = JSON.stringify(tree);
    rehypeCodeLines()(tree);

    expect(JSON.stringify(tree)).toBe(before);
  });

  it('numbers a diagram like any other code, since its code view is code', () => {
    // The diagram itself is drawn from the chart string, but the view a reader can
    // switch to is a code view, so it gets the shared treatment: numbered lines
    // and highlighting. Splitting the fence must still leave the chart readable,
    // which is what the text below checks.
    const tree: TestNode = {
      type: 'root',
      children: [
        {
          type: 'element',
          tagName: 'pre',
          children: [
            {
              type: 'element',
              tagName: 'code',
              properties: { className: ['language-mermaid'] },
              children: [{ type: 'text', value: 'graph LR\n  A --> B\n' }],
            },
          ],
        },
      ],
    };
    rehypeCodeLines()(tree);
    const code = tree.children![0].children![0];

    expect(code.children).toHaveLength(2);
    expect(code.children!.every(isLine)).toBe(true);
    // The chart the renderer is handed must match the source: splitting moves the
    // line breaks into the structure, and the diagram is parsed from this text,
    // so anything lost here is a chart that will not draw. The trailing newline
    // goes the way it does for every other fence, which mermaid does not care
    // about.
    expect(textOf(code)).toBe('graph LR\n  A --> B');
  });
});

describe('Markdown', () => {
  it('renders headings at a size distinct from body text', () => {
    // Preflight collapses headings onto the body size; the class carries the
    // scale, so this guards the hook rather than the pixels.
    const html = render('# One\n\n## Two\n\nplain text');

    expect(html).toContain('<h1');
    expect(html).toContain('<h2');
    expect(html).toContain('plain text');
  });

  it('keeps list markers, which preflight removes', () => {
    const html = render('- apples\n- oranges\n\n1. first\n2. second');

    expect(html).toMatch(/<ul[^>]*>/);
    expect(html).toMatch(/<ol[^>]*>/);
    expect(html).toContain('apples');
    expect(html).toContain('first');
  });

  it('renders GFM tables inside a scroll container', () => {
    const html = render('| Name | Qty |\n| --- | --- |\n| Coffee | 2 |');

    expect(html).toContain('table-scroll');
    expect(html).toContain('<table');
    expect(html).toContain('<th');
    expect(html).toContain('Coffee');
  });

  it('labels a fenced block with its language', () => {
    const html = render('```python\nprint(1)\n```');

    expect(html).toContain('code-block');
    expect(html).toMatch(/class="code-block-lang">python</);
  });

  it('labels an unlabelled fence so every block keeps the same shape', () => {
    // A bar that comes and goes with the language makes the transcript ragged,
    // so a fence that named nothing still gets a title.
    const html = render('```\nplain\n```');

    expect(html).toMatch(/class="code-block-lang">text</);
    expect(html).toContain('code-block-bar');
  });

  it('highlights code, so the fence language is recognised', () => {
    const html = render('```python\ndef f():\n    return 1\n```');

    expect(html).toContain('hljs');
    expect(html).toMatch(/class="hljs-keyword"/);
  });

  it('renders an unlabelled fence without inventing a language', () => {
    const html = render('```\nplain text\n```');

    expect(html).toContain('code-block');
    expect(html).not.toContain('hljs-keyword');
  });

  it('leaves an unknown language unhighlighted rather than failing', () => {
    // Models emit fences for languages highlight.js has never heard of.
    const html = render('```notalanguage\nsome content\n```');

    expect(html).toContain('some content');
  });

  it('renders a half-streamed fence while text is still arriving', () => {
    const html = render('```python\ndef f():');

    expect(html).toContain('python');
    expect(html).toContain('def');
  });

  it('renders raw HTML from model output as inert text', () => {
    const html = render('hello <script>alert(1)</script> <img src=x onerror=alert(1)>');

    // Escaped rather than dropped: the model wrote it, so showing the user what
    // arrived is more useful than silently losing it. What matters is that it
    // came out as text — the payload appearing escaped is the safe outcome, so
    // the assertions are on the absence of live markup, not of the substring.
    expect(html).not.toContain('<script');
    expect(html).not.toContain('<img');
    expect(html).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
  });

  it('copies the code alone, not the label or button beside it', () => {
    // Highlighting rewrites the fence into spans before `pre` sees it, so the
    // text is recovered by walking the tree. If that walk were wrong the button
    // would copy chrome or an empty string instead of the code.
    const html = render('```python\ndef f(x):\n    return 1\n```');
    const pre = html.slice(html.indexOf('<pre>'));

    expect(pre).not.toContain('Copy code');
    expect(pre).not.toContain('>python<');
    expect(pre).toContain('hljs');
  });

it('numbers one element per line, so the counter has something to count', () => {
    const html = render('```python\na = 1\nb = 2\nc = 3\n```');
    const lines = html.match(/class="code-line"/g) ?? [];

    expect(lines).toHaveLength(3);
  });

it('does not number a line past a trailing newline', () => {
  // Every fenced block ends in a newline; counting it would leave the last line
  // of every block numbered one higher than it is.
  const html = render('```\na\nb\n```');

  expect(html.match(/class="code-line"/g) ?? []).toHaveLength(2);
});

it('keeps a blank line, which still carries a number', () => {
  const html = render('```\na\n\nb\n```');

  expect(html.match(/class="code-line"/g) ?? []).toHaveLength(3);
});

it('preserves the newlines that line splitting removed', () => {
  // Splitting moves the breaks from the text into the structure. Reading the
  // code back without putting them back would copy the block as one long line.
  const html = render('```python\ndef f(x):\n    return 1\n```');
  const pre = html.slice(html.indexOf('<pre>'));

  // Three lines means two breaks: the highlight spans and line wrappers are the
  // only markup between the text.
  expect(pre).not.toContain('def f(x):    return 1');
  expect(pre).toContain('def');
  expect(pre).toContain('return');
});

it('splits a line break that falls inside a multi-line string', () => {
  // Highlighting wraps tokens in spans, so a break inside one has to split the
  // span too or half a token lands on the next line.
  const html = render('```python\nx = """\nmulti\n"""\n```');

  expect(html.match(/class="code-line"/g) ?? []).toHaveLength(3);
});

it('leaves inline code as a single element', () => {
  const html = render('use `a\nb` here');

  expect(html).not.toContain('code-line');
});

  it('keeps the highlighting when it splits', () => {
    // Splitting after highlighting must not unwrap the token spans. Losing them
    // leaves the code correct but entirely uncoloured, which no assertion on the
    // text alone would catch.
    const html = render('```python\ndef f():\n    return 1\n```');

    expect(html).toMatch(/class="hljs-keyword"/);
    expect(html).toMatch(/class="hljs-title/);
    // Each token sits inside a line rather than loose in the block.
    expect(html).toMatch(/class="code-line"><span class="hljs-keyword">def<\/span>/);
  });

  it('numbers every block from one, not continuously across the message', () => {
    // The counter is reset per block in CSS; two blocks in one message must not
    // run on into each other.
    const html = render('```\na\nb\n```\n\n```\nc\nd\n```');

    expect(html.match(/class="code-line"/g) ?? []).toHaveLength(4);
  });

  it('puts the language on the left and the copy button on the right', () => {
    const html = render('```python\nprint(1)\n```');
    const bar = html.slice(html.indexOf('code-block-bar'), html.indexOf('<pre>'));

    expect(bar.indexOf('code-block-lang')).toBeLessThan(bar.indexOf('<button'));
  });

  it('keeps the copy button visible without a hover', () => {
    // The button is the reason to title the block; hiding it until hover left the
    // header looking empty and the control looking absent.
    const html = render('```python\nprint(1)\n```');

    expect(html).not.toContain('opacity-0');
    expect(html).toContain('Copy code');
  });

  it('renders math rather than leaving the delimiters visible', () => {
    const html = render('inline $E = mc^2$ done');

    expect(html).toContain('katex');
    expect(html).not.toContain('$E');
  });
});
