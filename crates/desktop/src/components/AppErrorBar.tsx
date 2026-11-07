// The bar above the transcript for a failure that belongs to the app: a start
// or a load that did not happen. A failure inside a turn is shown with that
// turn instead.
import { TriangleAlert, X } from 'lucide-react';
import { t } from '@/lib/i18n';

export function AppErrorBar({
  message,
  onRetry,
  onDismiss,
}: {
  message: string;
  onRetry: () => void;
  onDismiss: () => void;
}) {
  return (
    <div className="flex items-center gap-2 border-b border-border bg-destructive/5 px-5 py-2 text-xs text-destructive">
      <TriangleAlert className="h-3.5 w-3.5 shrink-0" />
      <span className="min-w-0 flex-1 truncate" title={message}>
        {message}
      </span>
      <button
        type="button"
        onClick={onRetry}
        className="shrink-0 cursor-pointer rounded px-1.5 py-0.5 hover:bg-item-hover"
      >
        {t('app.retry')}
      </button>
      <button
        type="button"
        onClick={onDismiss}
        title={t('common.close')}
        aria-label={t('common.close')}
        className="shrink-0 cursor-pointer rounded p-0.5 hover:bg-item-hover"
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}
