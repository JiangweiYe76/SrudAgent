// One labelled side of a tool call: what went in, or what came back.
//
// The label is what makes the pair readable. A tool call has two payloads and
// they look alike — both are a block of JSON — so a reader who opened a call to
// check what the agent asked for has to be told which one they are looking at.
import { readJson } from '@/lib/json';
import { renderHighlighted } from '@/lib/highlightTree';
import { CopyCode } from '@/components/CodeBlockBar';
import { t } from '@/lib/i18n';

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
  const payload = empty
    ? { text: '', tree: null, unparsable: false }
    : readJson(value);
  // A failure is prose by design, so it is shown as prose. A success that is not
  // JSON is not prose at all — it is a payload that lost its structure, and saying
  // so beats colouring a sentence as though it were an object.
  const asProse = isError === true || (isError === undefined && payload.unparsable);
  // A failure's own words are what it is: re-indenting them would reflow prose
  // that was written to be read, and it would imply the text had been parsed.
  const displayed = asProse && isError === true ? value : payload.text;

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex items-center gap-2">
        <span className="text-[11px] text-muted-foreground">{label}</span>
        {isError === false && payload.unparsable && (
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
        {!empty && <CopyCode text={displayed} />}
      </div>
      {empty ? (
        <span className="font-mono break-words whitespace-pre-wrap text-muted-foreground">
          {fallback}
        </span>
      ) : asProse ? (
        <pre
          className={`m-0 font-mono break-words whitespace-pre-wrap ${
            isError === true ? 'text-destructive' : ''
          }`}
        >
          {displayed}
        </pre>
      ) : (
        <pre className="m-0 font-mono break-words whitespace-pre-wrap">
          {payload.tree ? renderHighlighted(payload.tree) : payload.text}
        </pre>
      )}
    </div>
  );
}