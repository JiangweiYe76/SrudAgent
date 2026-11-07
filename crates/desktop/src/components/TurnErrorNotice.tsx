// The notice a failed turn leaves in the transcript: what happened, and what
// a reader can do about it.
import { TriangleAlert } from 'lucide-react';
import { Modal, ModalContent, ModalTrigger } from '@/components/ui/modal';
import { t } from '@/lib/i18n';
import type { TurnError } from '@/lib/types';

// The code the agent answers a failed turn with, whatever went wrong. The cause
// is in the message, so the notice reads it from there instead of from this.
const INTERNAL_ERROR = -32603;

type ErrorKind =
  | 'internal'
  | 'auth'
  | 'rate'
  | 'server'
  | 'context'
  | 'network'
  | 'unknown';

// The HTTP status the provider refused with, read out of its own words. The
// text is the one thing both paths carry — a live failure and a reloaded one —
// while the JSON-RPC code is always the same internal-error number.
function statusIn(message: string): number | undefined {
  const match = /\b(4\d{2}|5\d{2})\b/.exec(message);
  return match ? Number(match[1]) : undefined;
}

function kindOf(error: TurnError | undefined): ErrorKind {
  if (!error) return 'unknown';
  const message = error.message.toLowerCase();
  const status = statusIn(message);
  if (
    error.code === 401 ||
    error.code === 403 ||
    status === 401 ||
    status === 403
  )
    return 'auth';
  if (error.code === 429 || status === 429) return 'rate';
  if (
    (error.code !== undefined && error.code >= 500 && error.code < 600) ||
    (status !== undefined && status >= 500)
  ) {
    return 'server';
  }
  if (
    (error.code === 400 || status === 400) &&
    /context|token|length|too long/.test(message)
  ) {
    return 'context';
  }
  if (/network|timeout|timed out|connect|offline|dns/.test(message)) {
    return 'network';
  }
  if (error.code === INTERNAL_ERROR) return 'internal';
  return 'unknown';
}

function describe(error: TurnError | undefined): string {
  switch (kindOf(error)) {
    case 'auth':
      return t('turn.error.authTitle');
    case 'rate':
      return t('turn.error.rateTitle');
    case 'server':
      return t('turn.error.serverTitle');
    case 'context':
      return t('turn.error.contextTitle');
    case 'network':
      return t('turn.error.networkTitle');
    case 'internal':
      return t('turn.error.title');
    default:
      return t('turn.error.unknownTitle');
  }
}

export function TurnErrorNotice({
  error,
  onRetry,
}: {
  // Absent when the turn was reloaded from a log that carried no error text.
  error?: TurnError;
  onRetry?: () => void;
}) {
  const title = describe(error);

  return (
    <div
      role="alert"
      className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded-xl border border-border px-3.5 py-2.5 text-[13px] leading-normal"
    >
      <TriangleAlert className="h-3.5 w-3.5 shrink-0 text-destructive" />
      <span className="text-foreground">{title}</span>

      <span className="ml-auto flex items-center gap-2">
        {onRetry && (
          <button
            type="button"
            onClick={onRetry}
            className="cursor-pointer text-[12px] text-muted-foreground transition-colors hover:text-foreground hover:underline"
          >
            {t('turn.retry')}
          </button>
        )}
        {error && (
          <Modal>
            <ModalTrigger asChild>
              <button
                type="button"
                className="cursor-pointer text-[12px] text-muted-foreground transition-colors hover:text-foreground hover:underline"
              >
                {t('turn.error.details')}
              </button>
            </ModalTrigger>
            <ModalContent
              title={t('turn.error.detailsTitle')}
              className="max-h-[80vh] w-[min(560px,90vw)]"
            >
              <div className="min-h-0 flex-1 overflow-y-auto p-4">
                <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-2 text-xs">
                  {error.code !== undefined && (
                    <>
                      <dt className="text-muted-foreground">
                        {t('turn.error.code')}
                      </dt>
                      <dd className="m-0 font-mono select-text">
                        {error.code}
                      </dd>
                    </>
                  )}
                  <dt className="text-muted-foreground">
                    {t('turn.error.message')}
                  </dt>
                  <dd className="m-0 font-mono break-words whitespace-pre-wrap select-text">
                    {error.message}
                  </dd>
                </dl>
              </div>
            </ModalContent>
          </Modal>
        )}
      </span>
    </div>
  );
}
