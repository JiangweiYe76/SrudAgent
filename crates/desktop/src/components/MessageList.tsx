import { memo, useEffect, useRef, useState } from 'react';
import {
  Brain,
  Check,
  ChevronDown,
  Copy,
  Loader2,
  RotateCcw,
  Wrench,
} from 'lucide-react';
import type { Attachment, Step, Turn } from '@/lib/types';
import { t } from '@/lib/i18n';
import { Markdown } from '@/components/Markdown';
import { JsonBlock } from '@/components/JsonBlock';
import { DiffBlock } from '@/components/DiffBlock';
import { MessageContextMenu } from '@/components/MessageContextMenu';
import { Modal, ModalContent } from '@/components/ui/modal';
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from '@/components/ui/collapsible';

interface MessageListProps {
  turns: Turn[];
  // True while a session's log is being replayed into the store, which is what
  // tells an empty conversation apart from one that has not arrived yet.
  reopening: boolean;
  // Room kept under the last message for the composer, which floats over the
  // list. Measured from the composer itself: queued messages make it taller
  // than any fixed padding would leave room for.
  bottomInset?: number;
  // Asks the model again for a finished turn, in that turn's place. The viewer
  // offers it on the last turn only: the turn after a discard begins again in
  // the slot the discarded one held, so there is never an older answer underneath
  // to keep.
  onRegenerate?: (turn: Turn) => void;
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

// Copy icon, an optional regenerate control and HH:MM, shown under a finished
// message.
interface MetaRowProps {
  text: string;
  ts: number;
  className?: string;
  // Given on a turn that can be asked again; absent renders no control.
  onRegenerate?: () => void;
}

function MetaRow({ text, ts, className = '', onRegenerate }: MetaRowProps) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <div
      className={`flex items-center gap-2 text-[11px] text-muted-foreground ${className}`}
    >
      <button
        className="flex cursor-pointer items-center rounded p-0.5 transition-colors hover:bg-item-hover hover:text-foreground"
        onClick={copy}
        title={t('msg.copy')}
      >
        {copied ? (
          <Check className="h-3.5 w-3.5" />
        ) : (
          <Copy className="h-3.5 w-3.5" />
        )}
      </button>
      {onRegenerate && (
        <button
          className="flex cursor-pointer items-center rounded p-0.5 transition-colors hover:bg-item-hover hover:text-foreground"
          onClick={onRegenerate}
          title={t('turn.regenerate')}
          aria-label={t('turn.regenerate')}
        >
          <RotateCcw className="h-3.5 w-3.5" />
        </button>
      )}
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
          <span className="shrink-0 text-muted-foreground">
            {t('thought.title')}
          </span>
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

// Memoised because `App` re-renders on every store write — a rename, another
// session's streaming, a batched replay flush — and re-drawing the transcript
// each time is what makes a long conversation expensive to keep on screen.
// Nothing here is read from outside its props, so the same `turns` reference
// means there is nothing new to draw.
export const MessageList = memo(function MessageList({
  turns,
  reopening,
  bottomInset = 140,
  onRegenerate,
}: MessageListProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const followingRef = useRef(true);
  // The image opened from a message, shown large over the transcript.
  const [preview, setPreview] = useState<Attachment | null>(null);

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

  // An empty pane is two things at once: a conversation nobody has had yet, and
  // one still on its way in. Saying "start a new chat" to a session mid-replay
  // would claim it is empty, which is the opposite of what is happening.
  if (turns.length === 0) {
    return (
      <div ref={listRef} className="flex flex-1 flex-col overflow-y-auto">
        <div className="m-auto flex items-center gap-2 text-sm text-muted-foreground">
          {reopening && (
            <Loader2
              className="h-4 w-4 animate-spin"
              role="status"
              aria-live="polite"
            />
          )}
          <span>
            {t(reopening ? 'empty.loadingSession' : 'empty.startChat')}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div ref={listRef} className="flex flex-1 flex-col overflow-y-auto">
      <div
        ref={contentRef}
        className="mx-auto flex w-full max-w-[900px] flex-col gap-4 p-[20px_32px_0]"
        style={{ paddingBottom: bottomInset }}
      >
        {turns.map((turn, index) => {
          const lastText =
            turn.steps[turn.steps.length - 1]?.assistantText ?? '';
          // Whether there is anything to copy or time-stamp. A turn can carry
          // only the question, or nothing at all — both are what a re-answer
          // leaves behind in the log — and neither has an answer of its own to
          // put a meta row under.
          const hasOutput = lastText !== '' || turn.steps.length > 0;
          // A turn can be asked again once it has ended with nothing running
          // after it. That covers a failed one too: the failure is the answer
          // to replace, not a reason the question is gone.
          const regenerable =
            onRegenerate !== undefined &&
            turn.endReason !== undefined &&
            index === turns.length - 1;
          return (
            <div key={turn.id} className="flex flex-col gap-3">
              {/* Each user message right-aligns inside the full column width:
                  the block stays stretched, so the bubble's `max-w-[80%]`
                  resolves against the column instead of against a
                  shrink-to-fit parent — which would pin the bubble to 80% of
                  its own meta row and wrap short messages. */}
              <div className="flex flex-col items-end gap-2">
                <div className="group flex w-full flex-col items-end gap-1">
                  {turn.images && turn.images.length > 0 && (
                    <div className="flex max-w-[80%] flex-wrap justify-end gap-2">
                      {turn.images.map((image) => (
                        <button
                          key={image.id}
                          type="button"
                          onClick={() => setPreview(image)}
                          title={t('chat.image.preview')}
                          aria-label={t('chat.image.preview')}
                          className="h-24 w-24 cursor-zoom-in overflow-hidden rounded-lg border border-border"
                        >
                          <img
                            src={`data:${image.mime};base64,${image.data}`}
                            alt=""
                            className="h-full w-full object-cover"
                          />
                        </button>
                      ))}
                    </div>
                  )}
                  {/* The bubble and its hover meta belong together: a turn
                      that carries no question of its own — the one a re-answer
                      left the answer on — has nothing for either to show. */}
                  {turn.userInput && (
                    <>
                      <MessageContextMenu text={turn.userInput}>
                        <div className="max-w-[80%] rounded-xl bg-user-bubble px-3.5 py-2.5 text-sm leading-normal break-words whitespace-pre-wrap">
                          {turn.userInput}
                        </div>
                      </MessageContextMenu>
                      <MetaRow
                        text={turn.userInput}
                        ts={turn.createdAt}
                        className="opacity-0 transition-opacity group-hover:opacity-100"
                      />
                    </>
                  )}
                </div>
                {/* A steered message is known per turn, not per step: the store
                    does not record which step was running when it arrived, so
                    the group is drawn after the turn's own input and before
                    its steps, and its meta carries the turn's start time. */}
                {(turn.steeredInputs ?? []).map((text, index) => (
                  <div
                    key={index}
                    className="group flex w-full flex-col items-end gap-1"
                  >
                    <MessageContextMenu text={text}>
                      <div className="max-w-[80%] rounded-xl bg-user-bubble px-3.5 py-2.5 text-sm leading-normal break-words whitespace-pre-wrap">
                        {text}
                      </div>
                    </MessageContextMenu>
                    <MetaRow
                      text={text}
                      ts={turn.createdAt}
                      className="opacity-0 transition-opacity group-hover:opacity-100"
                    />
                  </div>
                ))}
              </div>
              {turn.steps.map((step) => (
                <div key={step.id} className="flex flex-col gap-2">
                  {step.thought && (
                    <ThoughtBlock
                      thought={step.thought}
                      live={isThinking(turn, step)}
                    />
                  )}
                  {step.assistantText && (
                    <MessageContextMenu text={step.assistantText}>
                      <div>
                        <Markdown streaming={turn.endReason === undefined}>
                          {step.assistantText}
                        </Markdown>
                      </div>
                    </MessageContextMenu>
                  )}
                  {step.toolCalls.map((tc) => (
                    /* A call that reports file changes opens on them: the diff
                       is what the call did, and a reader should not have to
                       dig for it. */
                    <Collapsible
                      key={tc.id}
                      defaultOpen={(tc.diffs?.length ?? 0) > 0}
                    >
                      <div className="rounded-lg border border-border bg-muted text-[12.5px]">
                        <CollapsibleTrigger className="flex w-full cursor-pointer items-center gap-2 px-3 py-2 text-left">
                          <Wrench className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                          <span className="font-mono shrink-0 text-foreground">
                            {tc.title ?? tc.name}
                          </span>
                          {/* The same mark the thinking header uses, and in the same
                              place: a spacer, then the dots against the chevron.
                              A call whose arguments are a whole file is most of
                              the wait, and there is nothing to show for it until
                              they land. */}
                          {tc.pending ? (
                            <>
                              <span className="flex-1" />
                              <ThinkingDots />
                            </>
                          ) : (
                            <span className="min-w-0 flex-1 truncate text-muted-foreground">
                              {tc.args}
                            </span>
                          )}
                          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform [[data-state=open]_&]:rotate-180" />
                        </CollapsibleTrigger>
                        {/* Both sides, because the collapsed row shows the arguments and
                            nothing else: a reader who opens a call to see what
                            came back finds only what went in. The two payloads
                            are both JSON and look alike, so each is labelled. */}
                        {/* What changed first, then the raw payloads of how. */}
                        <CollapsibleContent className="flex flex-col gap-3 border-t border-border px-3 py-2">
                          {tc.diffs?.map((diff, i) => (
                            <DiffBlock key={`${i}-${diff.path}`} diff={diff} />
                          ))}
                          <JsonBlock
                            label={t('toolCall.input')}
                            value={tc.args}
                          />
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
              {/* A turn with nothing to show still gets its row when it can be
                  asked again: an answer that came back empty is the one most
                  worth retrying, and hiding the control would leave no way to. */}
              {turn.endReason !== undefined && (hasOutput || regenerable) && (
                <div className="flex items-center justify-between">
                  <MetaRow
                    text={lastText}
                    ts={turn.endedAt ?? turn.createdAt}
                    onRegenerate={
                      regenerable ? () => onRegenerate(turn) : undefined
                    }
                  />
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
      <Modal
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open) setPreview(null);
        }}
      >
        <ModalContent
          title={t('chat.image.preview')}
          className="max-h-[90vh] max-w-[90vw]"
          overlayClassName="bg-black/70"
        >
          {preview && (
            <img
              src={`data:${preview.mime};base64,${preview.data}`}
              alt={t('chat.image.preview')}
              className="max-h-[80vh] max-w-[80vw] object-contain"
            />
          )}
        </ModalContent>
      </Modal>
    </div>
  );
});
