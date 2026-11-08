import { memo, useMemo } from 'react';
import type { ToolDiff } from '@/lib/types';

// Lines past this are reported rather than drawn: a large file change would
// otherwise build a row for every line even though the card only shows a few.
const MAX_RENDER_LINES = 400;

/**
 * One file change a tool call reports, as a minus/plus diff.
 *
 * A created file has no text before it and draws no minus rows: a minus row
 * of nothing would read as a deletion.
 */
export const DiffBlock = memo(function DiffBlock({ diff }: { diff: ToolDiff }) {
  const removed = useMemo(
    () => (diff.oldText ? diff.oldText.split('\n') : []),
    [diff.oldText],
  );
  const added = useMemo(
    () => (diff.newText ? diff.newText.split('\n') : []),
    [diff.newText],
  );
  const shownRemoved = removed.slice(0, MAX_RENDER_LINES);
  const shownAdded = added.slice(0, MAX_RENDER_LINES);
  return (
    <div className="overflow-hidden rounded-md border border-border">
      <div className="truncate bg-muted px-2 py-1 font-mono text-[11px] text-muted-foreground">
        {diff.path}
      </div>
      <div className="overflow-x-auto font-mono text-[11.5px] leading-5">
        {shownRemoved.map((line, i) => (
          <div
            key={`-${i}`}
            className="whitespace-pre bg-destructive/10 text-destructive"
          >
            <span className="select-none px-2 opacity-60">-</span>
            {line}
          </div>
        ))}
        {removed.length > MAX_RENDER_LINES && (
          <div className="px-2 py-1 text-[11px] text-muted-foreground">
            … +{removed.length - MAX_RENDER_LINES} lines
          </div>
        )}
        {shownAdded.map((line, i) => (
          <div
            key={`+${i}`}
            className="whitespace-pre bg-success/10 text-success"
          >
            <span className="select-none px-2 opacity-60">+</span>
            {line}
          </div>
        ))}
        {added.length > MAX_RENDER_LINES && (
          <div className="px-2 py-1 text-[11px] text-muted-foreground">
            … +{added.length - MAX_RENDER_LINES} lines
          </div>
        )}
      </div>
    </div>
  );
});
