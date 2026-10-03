import { useEffect, useRef, useState } from 'react';
import { Brain, Check, ChevronDown, Copy, Wrench } from 'lucide-react';
import type { Step, Turn } from '@/lib/types';
import { t } from '@/lib/i18n';
import { Markdown } from '@/components/Markdown';
import { JsonBlock } from '@/components/JsonBlock';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';

interface MessageListProps {
  turns: Turn[];
}

// Wall-clock time of an event, 24-hour. The hour must be included: minutes
// alone repeat every hour, so `mm:ss` alone made a turn at 23:50 and one at
// 00:10 render as `50` and `10`.
function fmtTime(ts: number): string {
  const d = new Date(ts);
  const hh = String(d.getHours()).padStart(2, '0');
  const mm = String(d.getMinutes()).padStart(2, '0');
  return `${hh}:${mm}`;
}

// Copy icon + HH:MM timestamp shown under a finished message.
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

// The delay goes inline because the `animate-thinking-dot` utility sets the
// `animation` shorthand, which resets it.
function ThinkingDots() {
  return (
    <span className="flex shrink-0 items-center gap-[3px]" aria-hidden="true">
      {['0ms', '150ms', '300ms'].map((delay) => (
        <span
          key={delay}
          className="h-1 w-1 animate-thinking-dot rounded-full bg-accent opacity-[0.45]"
          style={{ animationDelay: delay }}
        />
      ))}
    </span>
  );
}

/**
 * Whether a turn has been sent but nothing has come back yet.
 *
 * This is the gap between submitting and the first token, which is where nothing
 * on screen has appeared yet and the reader has no way to tell the app heard them.
 *
 * A step is only opened when its first chunk lands, so a turn waiting on the model
 * normally has no steps at all. The contents are still checked rather than trusting
 * that, because a chunk carrying empty text can open a step with nothing in it.
 */
function awaitingFirstToken(turn: Turn): boolean {
  if (turn.endReason !== undefined) return false;
  return !turn.steps.some(
    (step) => step.thought || step.assistantText || step.toolCalls.length > 0,
  );
}

/**
 * Whether a step is still producing reasoning.
 *
 * A turn outlives its reasoning by most of its length: the model thinks, then writes
 * the answer, and only then does the turn close. So the turn being open says nothing
 * about whether thinking is still going on, and a mark tied to the turn kept
 * animating for the whole answer.
 *
 * Reasoning is over once the step has moved past it. Answer text in the same step
 * means the model has begun writing, and a tool call means it has left reasoning
 * altogether — the store starts a new step for reasoning that follows a call, so
 * tool calls here are the boundary between one sample and the next.
 */
function isThinking(turn: Turn, step: Step): boolean {
  if (turn.endReason !== undefined) return false;
  return !step.assistantText && step.toolCalls.length === 0;
}

// The model's reasoning, folded into a block above the answer it produced.
function ThoughtBlock({ thought, live }: { thought: string; live: boolean }) {
  const [open, setOpen] = useState(false);

  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <div className="rounded-lg border border-border bg-muted text-[12.5px]">
        <CollapsibleTrigger className="flex w-full cursor-pointer items-center gap-2 px-3 py-2 text-left">
          <Brain className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <span className="shrink-0 text-muted-foreground">{t('thought.title')}</span>
          <span className="flex-1" />
          {live && <ThinkingDots />}
          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform [[data-state=open]_&]:rotate-180" />
        </CollapsibleTrigger>
        <CollapsibleContent>
          <Markdown
            className="border-t border-border px-3 py-2 text-muted-foreground"
            streaming={live}
          >
            {thought}
          </Markdown>
        </CollapsibleContent>
      </div>
    </Collapsible>
  );
}

// How near the bottom still counts as following. A reader a few pixels short of
// the end has not scrolled away, and must not be treated as if they had.
const STICK_SLOP = 32;

export function MessageList({ turns }: MessageListProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const followingRef = useRef(true);

  // Whether the reader is at the bottom, tracked from their scrolling rather than
  // from the content growing. Somebody reading back through a long answer would
  // otherwise be dragged to the bottom by every token that arrived.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return;

    const onScroll = () => {
      const distance = list.scrollHeight - list.scrollTop - list.clientHeight;
      followingRef.current = distance <= STICK_SLOP;
    };
    list.addEventListener('scroll', onScroll, { passive: true });
    return () => list.removeEventListener('scroll', onScroll);
  }, []);

  // A new turn means the reader just sent something and wants to watch it arrive,
  // so following resumes even if they had scrolled back to read something.
  const turnCount = turns.length;
  const seenTurns = useRef(turnCount);
  useEffect(() => {
    if (turnCount > seenTurns.current) followingRef.current = true;
    seenTurns.current = turnCount;
  }, [turnCount]);

  // Follow the bottom, but only while following.
  //
  // `scrollTop` is assigned rather than reached with `scrollIntoView({ behavior:
  // 'smooth' })`, for two measured reasons. Smooth was re-targeted on every token
  // and never completed, leaving the view stalled short of the bottom it was meant
  // to be following — in a 60-chunk stream it moved nowhere at all. And
  // `scrollIntoView` scrolls every scrollable ancestor, not only this list. Setting
  // `scrollTop` lands on the new bottom in one step, which is what content growing
  // every few tens of milliseconds needs: there is nothing to animate towards.
  useEffect(() => {
    const list = listRef.current;
    if (list && followingRef.current) list.scrollTop = list.scrollHeight;
  }, [turns]);

  // Content that grows *after* the tokens that introduced it: a mermaid diagram
  // and a formula both render asynchronously and make the transcript taller once
  // they land. Following on `turns` alone would leave the reader short of the
  // bottom until they scrolled.
  useEffect(() => {
    const list = listRef.current;
    const content = contentRef.current;
    if (!list || !content) return;

    const follow = () => {
      if (followingRef.current) list.scrollTop = list.scrollHeight;
    };
    follow();

    const observer = new ResizeObserver(follow);
    observer.observe(content);
    return () => observer.disconnect();
  }, []);

  if (turns.length === 0) {
    return (
      <div ref={listRef} className="flex flex-1 flex-col overflow-y-auto">
        <div className="m-auto text-sm text-muted-foreground">{t('empty.startChat')}</div>
      </div>
    );
  }

  return (
    <div ref={listRef} className="flex flex-1 flex-col overflow-y-auto">
      <div
        ref={contentRef}
        className="mx-auto flex w-full max-w-[900px] flex-col gap-4 p-[20px_32px_140px]"
      >
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
                  {step.thought && (
                    <ThoughtBlock thought={step.thought} live={isThinking(turn, step)} />
                  )}
                  {step.assistantText && (
                    <Markdown streaming={turn.endReason === undefined}>
                      {step.assistantText}
                    </Markdown>
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
                        {/* Both sides, because the collapsed row shows the arguments and
                            nothing else: a reader who opens a call to see what
                            came back finds only what went in. The two payloads
                            are both JSON and look alike, so each is labelled. */}
                        <CollapsibleContent className="flex flex-col gap-3 border-t border-border px-3 py-2">
                          <JsonBlock label={t('toolCall.input')} value={tc.args} />
                          <JsonBlock
                            label={t('toolCall.output')}
                            value={tc.result ?? ''}
                            fallback={t('toolCall.noResult')}
                            isError={tc.isError}
                          />
                        </CollapsibleContent>
                      </div>
                    </Collapsible>
                  ))}
                </div>
              ))}
              {/* The wait for the model's first token, marked with the same dots the
                  thinking header uses rather than a second spinner. Once anything has
                  arrived the content itself is the progress indicator. */}
              {awaitingFirstToken(turn) && (
                <div
                  className="flex items-center py-1"
                  role="status"
                  aria-label={t('turn.awaitingReply')}
                >
                  <ThinkingDots />
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
      </div>
    </div>
  );
}
