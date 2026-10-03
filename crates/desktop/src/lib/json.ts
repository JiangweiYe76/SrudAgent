// A tool call's arguments and result, read as JSON.
//
// Both reach the UI as a flat string, and the only thing in it that says which
// key a value belongs to is the punctuation around it. Read as one line, that is
// something the reader has to supply themselves — and a payload whose values run
// together is exactly the one worth reading twice.
//
// Highlighting is done with lowlight directly rather than by handing the text to
// the markdown renderer inside a fence. A fence ends at the first ``` in its
// contents, and tool output is arbitrary text from the machine the agent runs
// on: the `read` tool pointed at a markdown file puts one there. The block would
// close early and the rest of the JSON would be rendered as prose.
import { createLowlight, common } from 'lowlight';
import type { HastNode } from './rehypeCodeLines';

const lowlight = createLowlight(common);

/** A JSON payload, and how to show it. */
export interface JsonPayload {
  /** The text to display: indented when it parsed, left as it came otherwise. */
  text: string;
  /**
   * The highlighted tree, or `null` when the text is not JSON.
   *
   * `null` is the honest answer rather than a best-effort tree: the grammar
   * colours commas as punctuation whether or not they separate anything, so
   * highlighting prose marks up punctuation in a sentence that has none, and the
   * result reads as structure that is not there. A tool that failed answers with
   * a message rather than a payload, and that message is shown as it was written.
   */
  tree: HastNode | null;
  /**
   * Whether the text failed to parse.
   *
   * Reported rather than left implied by `tree === null`, because the two reasons
   * a payload is not JSON are not the same. A tool that failed sends a message on
   * purpose; a tool that succeeded sends something that is not JSON by mistake,
   * and the second is a defect worth noticing rather than a shape to render.
   */
  unparsable: boolean;
}

/**
 * Reads a tool call's payload: indented, and highlighted when it is JSON.
 *
 * One `parse` decides both, so the text on screen and the colours on it cannot
 * end up disagreeing about whether this was JSON.
 *
 * Re-indenting is the other half of the work and the reason it is not just a
 * colouring pass: a payload arrives as one line, and a value cannot sit under the
 * key it belongs to when there is nothing to put it on.
 */
export function readJson(raw: string): JsonPayload {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { text: raw, tree: null, unparsable: true };
  }

  const text = JSON.stringify(parsed, null, 2);
  return { text, tree: lowlight.highlight('json', text) as unknown as HastNode, unparsable: false };
}