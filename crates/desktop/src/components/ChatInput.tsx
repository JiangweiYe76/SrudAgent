import { useEffect, useRef, useState } from 'react';

import { ArrowUp, Square, Trash2 } from 'lucide-react';
import { t } from '@/lib/i18n';
import type { UsageState } from '@/lib/types';
import { Button } from '@/components/ui/button';
import { UsageModal } from '@/components/UsageModal';

interface ChatInputProps {
  // The draft of the session on screen, and the way to change it.
  value: string;
  onValueChange: (text: string) => void;
  onSend: (text: string) => void;
  // Takes a message typed while a turn runs, moving it to the waiting list
  // instead of dropping it or opening a second turn.
  onQueue?: (text: string) => void;
  // Messages waiting for a turn, in the order they were queued.
  followups?: string[];
  // Injects a waiting message into the turn still running, and removes one
  // the user no longer wants to send.
  onSteerFollowup?: (index: number) => void;
  onRemoveFollowup?: (index: number) => void;
  // True while the session has a turn in flight. Stop is always rendered, so
  // the row does not shift when a turn starts; it is only clickable while busy.
  busy: boolean;
  onStop: () => void;
  // Token usage of the session on screen. Absent until a step reports any —
  // a model with no configured window shows nothing here.
  usage?: UsageState;
  // Reports the composer's height, so the conversation can keep enough room
  // under its last message: the box floats over the list, and queued messages
  // make it taller than any fixed padding allows for.
  onHeightChange?: (height: number) => void;
}

export function ChatInput({
  value,
  onValueChange,
  onSend,
  onQueue,
  followups,
  onSteerFollowup,
  onRemoveFollowup,
  busy,
  onStop,
  usage,
  onHeightChange,
}: ChatInputProps) {
  // The detail view behind the ring. It lives here so the indicator and its
  // modal come and go together with the usage data itself.
  const [usageOpen, setUsageOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const root = rootRef.current;
    if (!onHeightChange || !root) return;
    // Reported on every change, so a row joining or leaving the queue is
    // reflected at once rather than on the next resize.
    const report = () => onHeightChange(root.getBoundingClientRect().height);
    report();
    const observer = new ResizeObserver(report);
    observer.observe(root);
    return () => observer.disconnect();
  }, [onHeightChange]);

  const submit = () => {
    const text = value.trim();
    if (!text) return;
    // A message typed mid-turn joins the waiting list and leaves the box, so
    // the queue shows what is coming rather than the box holding it back.
    if (busy && onQueue) {
      onQueue(text);
      onValueChange('');
      return;
    }
    if (busy) return;
    // The store clears the draft when it takes the turn; a refused send leaves
    // the text in the box.
    onSend(text);
  };

  const queued = !!followups && followups.length > 0;

  // The composer itself: box, controls, and usage modal on one surface.
  const composer = (
    <div className="rounded-2xl border border-border bg-background p-[14px_16px_10px] shadow-[0_2px_12px_rgba(0,0,0,0.10)] focus-within:border-accent dark:shadow-[0_2px_12px_rgba(0,0,0,0.55)]">
      {/* A textarea defaults to its intrinsic `size=20` width, where the
          placeholder wraps to three lines and clips; `block w-full` stretches it
          to the card so the text lays out on its own terms. */}
      <textarea
        className="block w-full min-h-[44px] max-h-[200px] resize-none border-none bg-transparent text-sm leading-normal text-foreground outline-none placeholder:text-muted-foreground"
        value={value}
        placeholder={t('chat.placeholder')}
        rows={2}
        onChange={(e) => onValueChange(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !e.shiftKey) {
            e.preventDefault();
            submit();
          }
        }}
      />
      <div className="flex items-center justify-end gap-1.5">
        {usage && <UsageIndicator usage={usage} onOpen={() => setUsageOpen(true)} />}
        <Button
          variant="outline"
          size="icon"
          onClick={onStop}
          disabled={!busy}
          title={t('chat.stop')}
          aria-label={t('chat.stop')}
          // Red while it can be pressed, and back to the neutral outline once
          // there is nothing to stop. Coloured rather than plain outline because
          // stopping a running turn is the one control here that discards work
          // rather than adding it.
          className="text-destructive hover:bg-destructive/10 hover:text-destructive disabled:text-muted-foreground disabled:hover:bg-transparent disabled:hover:text-muted-foreground"
        >
          <Square className="h-3 w-3 fill-current" />
        </Button>
        <Button
          size="icon"
          onClick={submit}
          disabled={!value.trim() || busy}
          title={t('chat.send')}
          aria-label={t('chat.send')}
          className="disabled:bg-border disabled:text-muted-foreground"
        >
          <ArrowUp className="h-4 w-4" />
        </Button>
      </div>
      {usage && <UsageModal usage={usage} open={usageOpen} onOpenChange={setUsageOpen} />}
    </div>
  );

  return (
    <div
      ref={rootRef}
      className="absolute inset-x-0 bottom-[18px] z-10 mx-auto flex w-full max-w-[836px] flex-col gap-2"
    >
      {/* Waiting messages are inserted into the composer rather than floating
          above it: the rows and the box are the same width and sit flush, so
          the two read as a single control. The rows take the elevated surface
          behind them and the box keeps the page background, so the part you
          type into stays the brighter of the two. */}
      {queued ? (
        <div className="overflow-hidden rounded-2xl bg-muted">
          <div className="flex flex-col">
            {followups!.map((text, index) => (
              <div
                key={index}
                className="mx-4 flex items-center gap-2 border-b border-border py-2 last:border-b-0"
              >
                <span className="min-w-0 flex-1 truncate text-sm text-foreground">{text}</span>
                <button
                  type="button"
                  className="shrink-0 rounded-md p-1.5 text-muted-foreground hover:bg-item-hover hover:text-foreground disabled:opacity-30"
                  title={t('chat.steer')}
                  aria-label={t('chat.steer')}
                  disabled={!busy}
                  onClick={() => onSteerFollowup?.(index)}
                >
                  <ArrowUp className="h-4 w-4" />
                </button>
                <button
                  type="button"
                  className="shrink-0 rounded-md p-1.5 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                  title={t('common.delete')}
                  aria-label={t('common.delete')}
                  onClick={() => onRemoveFollowup?.(index)}
                >
                  <Trash2 className="h-4 w-4" />
                </button>
              </div>
            ))}
          </div>
          {composer}
        </div>
      ) : (
        composer
      )}
    </div>
  );
}

// Context pressure, beside the stop button.
//
// The ring fills as the context window is consumed; its tooltip carries the
// exact counts, and pressing it opens the full breakdown — cache hits and all.
function UsageIndicator({ usage, onOpen }: { usage: UsageState; onOpen: () => void }) {
  const ratio = usage.size > 0 ? Math.min(1, usage.used / usage.size) : 0;
  const circumference = 2 * Math.PI * 6;
  const contextTitle = t('chat.context', {
    pct: Math.round(ratio * 100),
    used: usage.used.toLocaleString(),
    size: usage.size.toLocaleString(),
  });
  return (
    <button
      type="button"
      onClick={onOpen}
      className="mr-1 flex cursor-pointer items-center gap-1.5 rounded-md px-1 py-0.5 text-[11px] text-muted-foreground transition-colors hover:bg-item-hover hover:text-foreground"
    >
      <svg width="16" height="16" viewBox="0 0 16 16" role="img" aria-label={contextTitle}>
        <title>{contextTitle}</title>
        <circle
          cx="8"
          cy="8"
          r="6"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          opacity="0.25"
        />
        {/* Starts at twelve and fills clockwise; the dash is the used share. */}
        <circle
          cx="8"
          cy="8"
          r="6"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeDasharray={`${circumference * ratio} ${circumference}`}
          transform="rotate(-90 8 8)"
        />
      </svg>
    </button>
  );
}
