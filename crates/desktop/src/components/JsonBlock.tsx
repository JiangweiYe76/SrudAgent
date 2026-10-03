// One labelled side of a tool call: what went in, or what came back.
//
// The label is what makes the pair readable. A tool call has two payloads and
// they look alike — both are a block of JSON — so a reader who opened a call to
// check what the agent asked for has to be told which one they are looking at.
import { readJson } from '@/lib/json';
import { renderHighlighted } from '@/lib/highlightTree';
import { CopyCode } from '@/components/CodeBlockBar';

/**
 * A labelled payload.
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
}: {
  label: string;
  value: string;
  fallback?: string;
}) {
  if (value.trim() === '' && fallback === undefined) return null;

  const empty = value.trim() === '';
  const { text, tree } = empty ? { text: '', tree: null } : readJson(value);

  return (
    <div className="flex min-w-0 flex-col gap-1">
      <div className="flex items-center gap-2">
        <span className="text-[11px] text-muted-foreground">{label}</span>
        <span className="flex-1" />
        {/* Only offered once there is something to copy: the button would
            otherwise copy an empty string and still claim it had. */}
        {!empty && <CopyCode text={text} />}
      </div>
      {empty ? (
        <span className="font-mono break-words whitespace-pre-wrap text-muted-foreground">
          {fallback}
        </span>
      ) : (
        <pre className="m-0 font-mono break-words whitespace-pre-wrap">
          {tree ? renderHighlighted(tree) : text}
        </pre>
      )}
    </div>
  );
}