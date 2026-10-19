import { useState } from 'react';

import { ArrowUp, Square } from 'lucide-react';
import { t } from '@/lib/i18n';
import type { UsageState } from '@/lib/types';
import { Button } from '@/components/ui/button';
import { UsageModal } from '@/components/UsageModal';

interface ChatInputProps {
  // The draft of the session on screen, and the way to change it.
  value: string;
  onValueChange: (text: string) => void;
  onSend: (text: string) => void;
  // True while the session has a turn in flight. Stop is always rendered, so
  // the row does not shift when a turn starts; it is only clickable while busy.
  busy: boolean;
  onStop: () => void;
  // Token usage of the session on screen. Absent until a step reports any —
  // a model with no configured window shows nothing here.
  usage?: UsageState;
}

export function ChatInput({ value, onValueChange, onSend, busy, onStop, usage }: ChatInputProps) {
  // The detail view behind the ring. It lives here so the indicator and its
  // modal come and go together with the usage data itself.
  const [usageOpen, setUsageOpen] = useState(false);

  const submit = () => {
    const text = value.trim();
    if (!text || busy) return;
    // The store clears the draft when it takes the turn; a refused send leaves
    // the text in the box.
    onSend(text);
  };

  return (
    <div className="absolute inset-x-0 bottom-[18px] z-10 mx-auto flex w-full max-w-[836px] flex-col gap-2 rounded-2xl border border-border bg-background p-[14px_16px_10px] shadow-[0_2px_12px_rgba(0,0,0,0.10)] focus-within:border-accent dark:shadow-[0_2px_12px_rgba(0,0,0,0.55)]">
      <textarea
        className="min-h-[44px] max-h-[200px] resize-none border-none bg-transparent text-sm leading-normal text-foreground outline-none placeholder:text-muted-foreground"
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
