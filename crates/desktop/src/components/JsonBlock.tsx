// One labelled side of a tool call: what went in, or what came back.
//
// The label is what makes the pair readable. A tool call has two payloads and
// they look alike — both are a block of JSON — so a reader who opened a call to
// check what the agent asked for has to be told which one they are looking at.
import { formatJson, highlightJson } from '@/lib/json';
import { renderHighlighted } from '@/lib/highlightTree';
import { CopyCode } from '@/components/CodeBlockBar';
import { Modal, ModalContent, ModalTrigger } from '@/components/ui/modal';
import { t } from '@/lib/i18n';

/**
 * How much of a payload is worth drawing.
 *
 * The card around this block scrolls, so the height on screen is bounded either
 * way. What is not bounded is the cost of laying out and highlighting a payload
 * that is mostly off screen: a long tool result would still build a DOM node for
 * every line. Past this limit the payload is shown as plain text, and the rest is
 * reported rather than drawn.
 */
const MAX_RENDER_LINES = 400;
const MAX_RENDER_CHARS = 32 * 1024;

interface TruncatedText {
  text: string;
  omittedLines: number;
}

/** Number of lines, counted without splitting the string into an array. */
function countLines(text: string): number {
  let lines = 1;
  let index = text.indexOf('\n');
  while (index !== -1) {
    lines += 1;
    index = text.indexOf('\n', index + 1);
  }
  return lines;
}

/**
 * The head of a payload, plus how many lines were left out.
 *
 * Both limits apply: a single long line stops at the character limit while the
 * line count reports nothing missing, and many short lines stop at the line limit
 * while the character limit is nowhere near reached.
 */
function truncateForRender(text: string): TruncatedText | null {
  const totalLines = countLines(text);
  if (totalLines <= MAX_RENDER_LINES && text.length <= MAX_RENDER_CHARS) {
    return null;
  }

  let cut = Math.min(text.length, MAX_RENDER_CHARS);
  let shownLines = countLines(text.slice(0, cut));

  if (totalLines > MAX_RENDER_LINES) {
    let newline = -1;
    for (let line = 0; line < MAX_RENDER_LINES; line += 1) {
      newline = text.indexOf('\n', newline + 1);
      if (newline === -1) break;
    }
    if (newline !== -1 && newline < cut) {
      cut = newline;
      shownLines = MAX_RENDER_LINES;
    }
  }

  return { text: text.slice(0, cut), omittedLines: totalLines - shownLines };
}

/**
 * A labelled payload.
 *
 * `isError` decides how the value is shown, rather than whether it happens to
 * parse: a tool that failed answers in prose and one that succeeded answers in
 * JSON, so the two are told apart by the backend's account of the call, not by
 * asking whether the text looks like JSON. A success that does not parse is a
 * tool breaking its own contract, and is marked rather than quietly rendered as
 * prose.
 *
 * `fallback` is what to show in place of a payload that has not arrived. A block
 * with neither payload nor fallback renders nothing at all: a tool that takes no
 * arguments would otherwise leave an "Input" label hanging over an empty space,
 * which reads as something that failed to load rather than as nothing to show.
 */
export function JsonBlock({
  label,
  value,
  fallback,
  isError,
}: {
  label: string;
  value: string;
  fallback?: string;
  isError?: boolean;
}) {
  if (value.trim() === '' && fallback === undefined) return null;

  const empty = value.trim() === '';
  const formatted = empty ? { text: '', unparsable: false } : formatJson(value);
  // A failure is prose by design, so it is shown as prose. A success that is not
  // JSON is not prose at all — it is a payload that lost its structure, and saying
  // so beats colouring a sentence as though it were an object.
  const asProse =
    isError === true || (isError === undefined && formatted.unparsable);
  // A failure's own words are what it is: re-indenting them would reflow prose
  // that was written to be read, and it would imply the text had been parsed.
  const base = asProse && isError === true ? value : formatted.text;
  const truncated = empty ? null : truncateForRender(base);
  const displayed = truncated ? truncated.text : base;
  // Text past the rendering limit is shown as written: highlighting a head that
  // was cut mid-payload would mark a structure that is no longer there.
  const tree =
    !truncated && !asProse && !formatted.unparsable
      ? highlightJson(base)
      : null;

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex items-center gap-2">
        <span className="text-[11px] text-muted-foreground">{label}</span>
        {isError === false && formatted.unparsable && (
          <span
            className="text-[11px] text-destructive"
            title="This tool reported success, so its output should have been JSON. It was not, so it is shown as written."
          >
            {t('toolCall.unexpectedText')}
          </span>
        )}
        <span className="flex-1" />
        {/* Only offered once there is something to copy: the button would
            otherwise copy an empty string and still claim it had. */}
        {!empty && <CopyCode text={base} />}
      </div>
      {empty ? (
        <span className="font-mono break-words whitespace-pre-wrap text-muted-foreground">
          {fallback}
        </span>
      ) : (
        <>
          <pre
            className={`m-0 font-mono break-words whitespace-pre-wrap ${
              isError === true ? 'text-destructive' : ''
            }`}
          >
            {tree ? renderHighlighted(tree) : displayed}
          </pre>
          {truncated && (
            <div className="flex items-center gap-2">
              <span className="font-mono text-[11px] text-muted-foreground">
                {truncated.omittedLines > 0
                  ? `… +${truncated.omittedLines} lines`
                  : '…'}
              </span>
              {/* The card is bounded, but the payload is not lost: the rest is
                  one click away instead of one scroll away. */}
              <Modal>
                <ModalTrigger asChild>
                  <button
                    type="button"
                    className="cursor-pointer text-[11px] text-muted-foreground underline decoration-border underline-offset-2 hover:text-foreground"
                  >
                    {t('toolCall.viewFull')}
                  </button>
                </ModalTrigger>
                <ModalContent
                  title={label}
                  className="max-h-[85vh] w-[min(760px,90vw)]"
                >
                  <div className="min-h-0 flex-1 overflow-y-auto p-4">
                    <pre className="m-0 font-mono text-[11.5px] leading-5 break-words whitespace-pre-wrap">
                      {base}
                    </pre>
                  </div>
                </ModalContent>
              </Modal>
            </div>
          )}
        </>
      )}
    </div>
  );
}
