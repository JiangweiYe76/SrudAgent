import { useEffect, useRef, useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { Check, ChevronDown, Circle, Copy, Wrench } from 'lucide-react';
import type { Turn } from '@/lib/types';
import { t } from '@/lib/i18n';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';

interface MessageListProps {
  turns: Turn[];
}

function fmtTime(ts: number): string {
  const d = new Date(ts);
  const mm = String(d.getMinutes()).padStart(2, '0');
  const ss = String(d.getSeconds()).padStart(2, '0');
  return `${mm}:${ss}`;
}

// Copy icon + mm:ss timestamp shown under a finished message.
function MetaRow({ text, ts, className = '' }: { text: string; ts: number; className?: string }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div className={`flex items-center gap-2 text-[11px] text-muted-foreground ${className}`}>
      <button
        className="flex cursor-pointer items-center rounded p-0.5 transition-colors hover:bg-item-hover hover:text-foreground"
        onClick={copy}
        title={t('msg.copy')}
      >
        {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
      </button>
      <span className="font-mono">{fmtTime(ts)}</span>
    </div>
  );
}

export function MessageList({ turns }: MessageListProps) {
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [turns]);

  if (turns.length === 0) {
    return (
      <div className="flex flex-1 flex-col overflow-y-auto">
        <div className="m-auto text-sm text-muted-foreground">{t('empty.startChat')}</div>
      </div>
    );
  }

  return (
    <div className="flex flex-1 flex-col overflow-y-auto">
      <div className="mx-auto flex w-full max-w-[900px] flex-col gap-4 p-[20px_32px_140px]">
        {turns.map((turn) => {
          const lastText = turn.steps[turn.steps.length - 1]?.assistantText ?? '';
          return (
            <div key={turn.id} className="flex flex-col gap-3">
              <div className="group flex flex-col items-end gap-1">
                <div className="max-w-[80%] rounded-xl bg-user-bubble px-3.5 py-2.5 text-sm leading-normal break-words whitespace-pre-wrap">
                  {turn.userInput}
                </div>
                {/* User msg meta appears only on hover. */}
                <MetaRow
                  text={turn.userInput}
                  ts={turn.createdAt}
                  className="opacity-0 transition-opacity group-hover:opacity-100"
                />
              </div>
              {turn.steps.map((step) => (
                <div key={step.id} className="flex flex-col gap-2">
                  {step.assistantText && (
                    <div className="message-markdown">
                      <ReactMarkdown remarkPlugins={[remarkGfm]}>{step.assistantText}</ReactMarkdown>
                    </div>
                  )}
                  {step.toolCalls.map((tc) => (
                    <Collapsible key={tc.id}>
                      <div className="rounded-lg border border-border bg-muted text-[12.5px]">
                        <CollapsibleTrigger className="flex w-full cursor-pointer items-center gap-2 px-3 py-2 text-left">
                          <Wrench className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="font-mono shrink-0 text-foreground">{tc.name}</span>
                          <span className="min-w-0 flex-1 truncate text-muted-foreground">{tc.args}</span>
                          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform [[data-state=open]_&]:rotate-180" />
                        </CollapsibleTrigger>
                        <CollapsibleContent>
                          <pre className="m-0 border-t border-border px-3 py-2 font-mono break-words whitespace-pre-wrap">
                            {tc.result ?? t('toolCall.noResult')}
                          </pre>
                        </CollapsibleContent>
                      </div>
                    </Collapsible>
                  ))}
                </div>
              ))}
              {turn.endReason === undefined && (
                <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Circle className="h-2 w-2 animate-pulse fill-accent text-accent" />
                  {t('turn.running')}
                </div>
              )}
              {turn.endReason && (
                <div className="flex items-center justify-between">
                  <MetaRow text={lastText} ts={turn.endedAt ?? turn.createdAt} />
                  {turn.endReason !== 'completed' && (
                    <div
                      className={`text-xs ${
                        turn.endReason === 'error'
                          ? 'text-destructive'
                          : turn.endReason === 'blocked'
                            ? 'text-warning'
                            : 'text-muted-foreground'
                      }`}
                    >
                      {t('turn.ended', { reason: turn.endReason })}
                    </div>
                  )}
                </div>
              )}
            </div>
          );
        })}
        <div ref={bottomRef} />
      </div>
    </div>
  );
}
