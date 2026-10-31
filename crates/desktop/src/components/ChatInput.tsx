import { useEffect, useRef, useState, type ClipboardEvent } from 'react';

import {
  ArrowUp,
  ClipboardPaste,
  Copy,
  Scissors,
  Square,
  TextCursorInput,
  Trash2,
  X,
} from 'lucide-react';
import { toast } from 'sonner';
import { t } from '@/lib/i18n';
import { isTauri, readClipboard, readImageFile } from '@/lib/acp';
import type { Attachment, UsageState } from '@/lib/types';
import { Button } from '@/components/ui/button';
import { ContextMenu } from '@/components/ContextMenu';
import { Modal, ModalContent } from '@/components/ui/modal';
import { UsageModal } from '@/components/UsageModal';

// Images are sent as base64 inside the prompt, so an oversized file is refused
// at the paste rather than growing the session log without bound.
const IMAGE_LIMIT_BYTES = 10 * 1024 * 1024;

let attachmentCounter = 0;

interface ChatInputProps {
  // The draft of the session on screen, and the way to change it.
  value: string;
  onValueChange: (text: string) => void;
  // Images pasted into the box, waiting for the next message.
  attachments: Attachment[];
  onAttachmentsChange: (attachments: Attachment[]) => void;
  onSend: (text: string, attachments: Attachment[]) => void;
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

/** The draft selection a right-click opened the editing menu over. */
interface DraftSelection {
  start: number;
  end: number;
  text: string;
}

export function ChatInput({
  value,
  onValueChange,
  attachments,
  onAttachmentsChange,
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
  const [preview, setPreview] = useState<Attachment | null>(null);
  const [draftSelection, setDraftSelection] = useState<DraftSelection | null>(
    null,
  );
  const rootRef = useRef<HTMLDivElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  // Reads started by a paste resolve after the event, so the list they append
  // to is the latest one rather than the one captured when the paste fired.
  const attachmentsRef = useRef(attachments);
  attachmentsRef.current = attachments;
  // The draft as it stands when a host read resolves, so text is written into
  // what the box holds now rather than what it held at the paste.
  const valueRef = useRef(value);
  valueRef.current = value;

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
    if (!text && attachments.length === 0) return;
    // A running turn takes text only: images wait in the box until it ends.
    if (busy && attachments.length > 0) return;
    // A message typed mid-turn joins the waiting list and leaves the box, so
    // the queue shows what is coming rather than the box holding it back.
    if (busy && onQueue) {
      onQueue(text);
      onValueChange('');
      return;
    }
    if (busy) return;
    // The store clears the draft and the images when it takes the turn; a
    // refused send leaves them in the box.
    onSend(text, attachments);
  };

  // Writes text the host read back into the box at the caret. Used only for a
  // paste the webview handed over empty and the composer had to take over.
  const insertText = (text: string) => {
    const textarea = textareaRef.current;
    const draft = valueRef.current;
    const start = Math.min(
      textarea?.selectionStart ?? draft.length,
      draft.length,
    );
    const end = Math.min(textarea?.selectionEnd ?? start, draft.length);
    onValueChange(`${draft.slice(0, start)}${text}${draft.slice(end)}`);
    const caret = start + text.length;
    // The draft owns the value, so the caret is placed once React has written
    // the new text into the box.
    requestAnimationFrame(() => textarea?.setSelectionRange(caret, caret));
  };

  // Writes over a remembered range and puts the caret after the new text. The
  // draft owns the value, so the caret moves once React has written it.
  const replaceDraftRange = (start: number, end: number, text: string) => {
    const draft = valueRef.current;
    const from = Math.min(start, draft.length);
    const to = Math.min(Math.max(end, from), draft.length);
    onValueChange(`${draft.slice(0, from)}${text}${draft.slice(to)}`);
    const caret = from + text.length;
    requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      textarea?.focus();
      textarea?.setSelectionRange(caret, caret);
    });
  };

  // Remembers the selection the way it stands at the right-click, because the
  // menu actions run after the pointer has already moved on.
  const openDraftMenu = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const draft = valueRef.current;
    const start = Math.min(textarea.selectionStart, draft.length);
    const end = Math.min(Math.max(textarea.selectionEnd, start), draft.length);
    setDraftSelection({
      start,
      end,
      text: draft.slice(start, end),
    });
  };

  // Choosing an item moves focus out of the draft, so the caret goes back to
  // the selection it acted on.
  const restoreSelection = (selection: DraftSelection) => {
    requestAnimationFrame(() => {
      const textarea = textareaRef.current;
      textarea?.focus();
      textarea?.setSelectionRange(selection.start, selection.end);
    });
  };

  const copySelection = async (selection: DraftSelection) => {
    try {
      await navigator.clipboard.writeText(selection.text);
    } catch {
      // Denied, or no clipboard available. Nothing was copied to report.
      return;
    } finally {
      restoreSelection(selection);
    }
    toast.success(t('msg.copied'));
  };

  const cutSelection = async (selection: DraftSelection) => {
    try {
      await navigator.clipboard.writeText(selection.text);
    } catch {
      // Denied, so the text stays in the draft rather than disappearing.
      restoreSelection(selection);
      return;
    }
    replaceDraftRange(selection.start, selection.end, '');
  };

  const pasteAtSelection = async (selection: DraftSelection) => {
    try {
      if (isTauri()) {
        const content = await readClipboard(IMAGE_LIMIT_BYTES);
        if (content.kind === 'text') {
          replaceDraftRange(selection.start, selection.end, content.text);
        } else if (content.kind === 'image') {
          onAttachmentsChange([
            ...attachmentsRef.current,
            attachmentOf(content),
          ]);
          restoreSelection(selection);
        } else {
          restoreSelection(selection);
        }
        return;
      }
      const text = await navigator.clipboard.readText();
      if (text) replaceDraftRange(selection.start, selection.end, text);
      else restoreSelection(selection);
    } catch (error: unknown) {
      const kind = (error as { kind?: string } | null)?.kind;
      if (kind === 'tooLarge') toast.error(t('chat.image.tooLarge'));
      else if (kind) toast.error(t('chat.image.unreadable'));
    }
  };

  const selectWholeDraft = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    textarea.focus();
    textarea.select();
  };

  const pasteImages = (event: ClipboardEvent<HTMLTextAreaElement>) => {
    const fromItems = Array.from(event.clipboardData.items)
      .filter((item) => item.kind === 'file' && item.type.startsWith('image/'))
      .map((item) => item.getAsFile())
      .filter((file): file is File => file !== null);
    // Some clipboards expose the files only through `files`, with no item for
    // each one.
    const files =
      fromItems.length > 0
        ? fromItems
        : Array.from(event.clipboardData.files).filter((file) =>
            file.type.startsWith('image/'),
          );
    if (files.length > 0) {
      // Only a clipboard carrying an image is taken over, so pasting text keeps
      // the browser's own handling.
      event.preventDefault();
      const accepted = files.filter((file) => {
        if (file.size <= IMAGE_LIMIT_BYTES) return true;
        toast.error(t('chat.image.tooLarge'));
        return false;
      });
      void Promise.all(accepted.map(readAttachment))
        .then((loaded) => {
          onAttachmentsChange([...attachmentsRef.current, ...loaded]);
        })
        .catch(() => toast.error(t('chat.image.unreadable')));
      return;
    }
    // File managers on Linux put a path on the clipboard instead of the bytes,
    // and the webview cannot open that path itself, so the host reads it.
    const path = clipboardImagePath(event.clipboardData);
    if (path && isTauri()) {
      event.preventDefault();
      void readImageFile(path, IMAGE_LIMIT_BYTES)
        .then((image) => {
          onAttachmentsChange([...attachmentsRef.current, attachmentOf(image)]);
        })
        .catch((error: unknown) => {
          const kind = (error as { kind?: string } | null)?.kind;
          toast.error(
            kind === 'tooLarge'
              ? t('chat.image.tooLarge')
              : t('chat.image.unreadable'),
          );
        });
      return;
    }
    // A paste can also reach the page with nothing usable on it — no image
    // bytes and no path — and the default handling then writes the path the
    // file was copied from into the box. The host reads the clipboard instead.
    if (!isTauri() || event.clipboardData.getData('text/plain')) return;
    event.preventDefault();
    void readClipboard(IMAGE_LIMIT_BYTES)
      .then((content) => {
        if (content.kind === 'image') {
          onAttachmentsChange([
            ...attachmentsRef.current,
            attachmentOf(content),
          ]);
        } else if (content.kind === 'text') {
          insertText(content.text);
        }
      })
      .catch((error: unknown) => {
        const kind = (error as { kind?: string } | null)?.kind;
        toast.error(
          kind === 'tooLarge'
            ? t('chat.image.tooLarge')
            : t('chat.image.unreadable'),
        );
      });
  };

  const queued = !!followups && followups.length > 0;

  // The composer itself: box, controls, and usage modal on one surface.
  const composer = (
    <div className="rounded-2xl border border-border bg-background p-[14px_16px_10px] shadow-[0_2px_12px_rgba(0,0,0,0.10)] focus-within:border-accent dark:shadow-[0_2px_12px_rgba(0,0,0,0.55)]">
      {attachments.length > 0 && (
        <div className="mb-2 flex flex-wrap gap-2">
          {attachments.map((image) => (
            <div
              key={image.id}
              className="group relative h-16 w-16 overflow-hidden rounded-lg border border-border"
            >
              <button
                type="button"
                onClick={() => setPreview(image)}
                title={t('chat.image.preview')}
                aria-label={t('chat.image.preview')}
                className="block h-full w-full cursor-zoom-in"
              >
                <img
                  src={imageSrc(image)}
                  alt=""
                  className="h-full w-full object-cover"
                />
              </button>
              <button
                type="button"
                onClick={() =>
                  onAttachmentsChange(
                    attachments.filter((item) => item.id !== image.id),
                  )
                }
                title={t('chat.image.remove')}
                aria-label={t('chat.image.remove')}
                className="absolute right-1 top-1 flex cursor-pointer items-center justify-center rounded-full bg-black/60 p-0.5 text-white opacity-0 transition-opacity group-hover:opacity-100 hover:bg-black/80"
              >
                <X className="h-3 w-3" />
              </button>
            </div>
          ))}
        </div>
      )}
      {/* A textarea defaults to its intrinsic `size=20` width, where the
          placeholder wraps to three lines and clips; `block w-full` stretches it
          to the card so the text lays out on its own terms. */}
      <ContextMenu
        label={t('edit.actions')}
        onOpen={openDraftMenu}
        items={[
          {
            label: t('edit.cut'),
            icon: Scissors,
            disabled: !draftSelection?.text,
            onSelect: () => {
              if (draftSelection) void cutSelection(draftSelection);
            },
          },
          {
            label: t('edit.copy'),
            icon: Copy,
            disabled: !draftSelection?.text,
            onSelect: () => {
              if (draftSelection) void copySelection(draftSelection);
            },
          },
          {
            label: t('edit.paste'),
            icon: ClipboardPaste,
            onSelect: () => {
              if (draftSelection) void pasteAtSelection(draftSelection);
            },
          },
          {
            label: t('edit.selectAll'),
            icon: TextCursorInput,
            onSelect: selectWholeDraft,
          },
        ]}
      >
        <textarea
          ref={textareaRef}
          className="block w-full min-h-[44px] max-h-[200px] resize-none border-none bg-transparent text-sm leading-normal text-foreground outline-none placeholder:text-muted-foreground"
          value={value}
          placeholder={t('chat.placeholder')}
          rows={2}
          onChange={(e) => onValueChange(e.target.value)}
          onPaste={pasteImages}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
        />
      </ContextMenu>
      <div className="flex items-center justify-end gap-1.5">
        {usage && (
          <UsageIndicator usage={usage} onOpen={() => setUsageOpen(true)} />
        )}
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
          disabled={(!value.trim() && attachments.length === 0) || busy}
          title={t('chat.send')}
          aria-label={t('chat.send')}
          className="disabled:bg-border disabled:text-muted-foreground"
        >
          <ArrowUp className="h-4 w-4" />
        </Button>
      </div>
      {usage && (
        <UsageModal
          usage={usage}
          open={usageOpen}
          onOpenChange={setUsageOpen}
        />
      )}
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
              src={imageSrc(preview)}
              alt={t('chat.image.preview')}
              className="max-h-[80vh] max-w-[80vw] object-contain"
            />
          )}
        </ModalContent>
      </Modal>
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
                <span className="min-w-0 flex-1 truncate text-sm text-foreground">
                  {text}
                </span>
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

function imageSrc(image: Attachment): string {
  return `data:${image.mime};base64,${image.data}`;
}

// An attachment out of image bytes the host read off the clipboard.
function attachmentOf(image: { mime: string; data: string }): Attachment {
  return {
    id: `img${attachmentCounter++}`,
    mime: image.mime,
    data: image.data,
  };
}

// The extensions the host will read off a path the clipboard named.
const IMAGE_PATH = /\.(png|jpe?g|gif|webp|bmp|svg|avif)$/i;

// The image file a clipboard names instead of carrying.
//
// A file manager copies a `text/uri-list` of `file://` entries, and some
// clipboards also offer the path as plain text, while the bytes are not
// available to the page. Only an entry naming an image is taken, so copying a
// folder or a document still pastes as the browser would.
function clipboardImagePath(data: DataTransfer): string | null {
  for (const line of data.getData('text/uri-list').split('\n')) {
    const uri = line.trim();
    if (!uri || uri.startsWith('#')) continue;
    const path = filePathOf(uri);
    if (path) return path;
  }
  for (const line of data.getData('text/plain').split('\n')) {
    const text = line.trim();
    const uriPath = filePathOf(text);
    if (uriPath) return uriPath;
    // An absolute path, so a sentence that happens to end in `.png` is left
    // alone.
    if (/^(\/|[A-Za-z]:[\\/])/.test(text) && IMAGE_PATH.test(text)) return text;
  }
  return null;
}

function filePathOf(uri: string): string | null {
  if (!uri.startsWith('file://')) return null;
  try {
    const path = decodeURIComponent(new URL(uri).pathname);
    // `file:///C:/x` names a drive path on Windows, where the leading slash
    // belongs to the URL rather than to the path.
    const local = /^\/[A-Za-z]:/.test(path) ? path.slice(1) : path;
    return IMAGE_PATH.test(local) ? local : null;
  } catch {
    return null;
  }
}

// Reads a pasted file as base64, which is what the prompt carries.
function readAttachment(file: File): Promise<Attachment> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(reader.error);
    reader.onload = () => {
      const url = String(reader.result ?? '');
      const comma = url.indexOf(',');
      if (comma < 0) {
        reject(new Error('the file was not read as a data URL'));
        return;
      }
      resolve({
        id: `img${attachmentCounter++}`,
        mime: file.type || 'image/png',
        data: url.slice(comma + 1),
      });
    };
    reader.readAsDataURL(file);
  });
}

// Context pressure, beside the stop button.
//
// The ring fills as the context window is consumed; its tooltip carries the
// exact counts, and pressing it opens the full breakdown — cache hits and all.
function UsageIndicator({
  usage,
  onOpen,
}: {
  usage: UsageState;
  onOpen: () => void;
}) {
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
      <svg
        width="16"
        height="16"
        viewBox="0 0 16 16"
        role="img"
        aria-label={contextTitle}
      >
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
