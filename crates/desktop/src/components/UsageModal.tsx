import { t } from '@/lib/i18n';
import type { UsageState } from '@/lib/types';
import { Modal, ModalContent } from '@/components/ui/modal';

interface UsageModalProps {
  usage: UsageState;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

// The full picture behind the ring: how much of the window is in use, how
// much of the input the cache served, and every accumulated count.
export function UsageModal({ usage, open, onOpenChange }: UsageModalProps) {
  const contextPct = usage.size > 0 ? Math.round((usage.used / usage.size) * 100) : 0;
  const cachePct =
    usage.inputTokens > 0
      ? Math.round((usage.cachedInputTokens / usage.inputTokens) * 100)
      : null;
  const rows: [string, number][] = [
    [t('usage.input'), usage.inputTokens],
    [t('usage.output'), usage.outputTokens],
    [t('usage.cached'), usage.cachedInputTokens],
    [t('usage.cacheWrite'), usage.cacheWriteInputTokens],
    [t('usage.reasoning'), usage.reasoningOutputTokens],
  ];
  return (
    <Modal open={open} onOpenChange={onOpenChange}>
      <ModalContent title={t('usage.title')} className="w-[360px]">
        <div className="flex flex-col p-4">
          <div className="grid grid-cols-2 gap-3">
            <Stat
              label={t('usage.context')}
              value={`${contextPct}%`}
              detail={`${usage.used.toLocaleString()} / ${usage.size.toLocaleString()}`}
            />
            <Stat
              label={t('usage.cacheHit')}
              value={cachePct === null ? '—' : `${cachePct}%`}
              detail={
                cachePct === null
                  ? undefined
                  : `${usage.cachedInputTokens.toLocaleString()} / ${usage.inputTokens.toLocaleString()}`
              }
            />
          </div>
          <div className="mt-3 flex flex-col">
            {rows.map(([label, value]) => (
              <div
                key={label}
                className="flex items-center justify-between border-b border-border/60 py-1.5 text-xs last:border-none"
              >
                <span className="text-muted-foreground">{label}</span>
                <span className="tabular-nums">{value.toLocaleString()}</span>
              </div>
            ))}
          </div>
          <p className="m-0 mt-3 text-[11px] text-muted-foreground">{t('usage.note')}</p>
        </div>
      </ModalContent>
    </Modal>
  );
}

function Stat({ label, value, detail }: { label: string; value: string; detail?: string }) {
  return (
    <div className="flex flex-col gap-0.5 rounded-lg bg-item-hover/50 px-3 py-2">
      <span className="text-[11px] text-muted-foreground">{label}</span>
      <span className="text-lg font-semibold tabular-nums">{value}</span>
      {detail && <span className="text-[11px] text-muted-foreground tabular-nums">{detail}</span>}
    </div>
  );
}
