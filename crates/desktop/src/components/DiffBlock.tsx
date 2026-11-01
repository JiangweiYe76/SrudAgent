import type { ToolDiff } from '@/lib/types';

/**
 * One file change a tool call reports, as a minus/plus diff.
 *
 * A created file has no text before it and draws no minus rows: a minus row
 * of nothing would read as a deletion.
 */
export function DiffBlock({ diff }: { diff: ToolDiff }) {
  const removed = diff.oldText ? diff.oldText.split('\n') : [];
  const added = diff.newText ? diff.newText.split('\n') : [];
  return (
    <div className="overflow-hidden rounded-md border border-border">
      <div className="truncate bg-muted px-2 py-1 font-mono text-[11px] text-muted-foreground">
        {diff.path}
      </div>
      <div className="overflow-x-auto font-mono text-[11.5px] leading-5">
        {removed.map((line, i) => (
          <div
            key={`-${i}`}
            className="whitespace-pre bg-destructive/10 text-destructive"
          >
            <span className="select-none px-2 opacity-60">-</span>
            {line}
          </div>
        ))}
        {added.map((line, i) => (
          <div
            key={`+${i}`}
            className="whitespace-pre bg-success/10 text-success"
          >
            <span className="select-none px-2 opacity-60">+</span>
            {line}
          </div>
        ))}
      </div>
    </div>
  );
}
