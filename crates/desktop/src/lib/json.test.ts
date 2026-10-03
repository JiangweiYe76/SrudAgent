// Exercises reading a tool call's payload as JSON.
//
// The two things worth proving are that a payload is actually coloured, and that
// one which is not JSON is left alone. The second is the one that cannot be
// checked by reading: the json grammar accepts prose and returns a tree for it,
// so a block that only checked "did this come back coloured" would pass while
// showing a sentence with its commas marked up as structure.
import { describe, it, expect } from 'vitest';
import { readJson } from './json';
import type { HastNode } from './rehypeCodeLines';

/** The highlight classes anywhere in a tree. */
function classesIn(node: HastNode): string[] {
  const className = node.properties?.className;
  const own = Array.isArray(className) ? className.map(String) : [];
  return [...own, ...(node.children ?? []).flatMap(classesIn)];
}

/** The text a tree renders back to, which must equal what `text` says. */
function textIn(node: HastNode): string {
  if (typeof node.value === 'string') return node.value;
  return (node.children ?? []).map(textIn).join('');
}

describe('readJson', () => {
  it('colours the keys, strings and numbers of a payload', () => {
    const { tree } = readJson('{"command":"ls -la","exit_code":0}');

    const classes = classesIn(tree!);
    expect(classes).toContain('hljs-attr');
    expect(classes).toContain('hljs-string');
    expect(classes).toContain('hljs-number');
  });

  it('indents so a value sits under its key', () => {
    // A payload arrives as one line, so this is the difference between a reader
    // finding the value by eye and scanning for it.
    const { text } = readJson('{"command":"ls -la"}');
    expect(text).toBe('{\n  "command": "ls -la"\n}');
  });

  it('keeps the highlighted text identical to the displayed text', () => {
    // The two are produced from one parse. If they were produced separately a
    // change to either would show a block whose colours sit on text that is not
    // what is on screen.
    const { text, tree } = readJson('{"a":[1,2],"b":{"c":null}}');
    expect(textIn(tree!)).toBe(text);
  });

  it.each([
    ['a refusal', 'bash is not available through this tool.'],
    ['the no-result placeholder', '(no result)'],
    ['an empty string', ''],
  ])('leaves %s alone', (_name, raw) => {
    // Prose has no keys, and the grammar would still hand back a tree for it —
    // one that colours commas in a sentence that has none.
    expect(readJson(raw).tree).toBeNull();
  });

  it('leaves a payload it cannot parse exactly as it came', () => {
    // Refusals are answers, not failures to answer: the text is what the agent
    // said, and re-indenting it would invent structure it does not have.
    const raw = 'the file has  no  tidy  shape';
    expect(readJson(raw).text).toBe(raw);
  });

  it('handles the booleans and null a tool result carries', () => {
    const { text, tree } = readJson('{"is_error":false,"truncated":null}');
    expect(textIn(tree!)).toBe(text);
    expect(text).toContain('"is_error": false');
    expect(text).toContain('"truncated": null');
  });

  it('survives the newlines a multi-line result carries', () => {
    // The read tool returns file content with its own line breaks inside it.
    // Highlighted text is split per line elsewhere in the app, so a break landing
    // inside a string is the case that could drop or duplicate content.
    const { text, tree } = readJson('{"content":"NAME=\\"Arch\\"\\nID=arch\\n"}');
    expect(textIn(tree!)).toBe(text);
    expect(classesIn(tree!)).toContain('hljs-string');
  });
});