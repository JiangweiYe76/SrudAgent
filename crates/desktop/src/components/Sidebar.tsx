import { Plus } from 'lucide-react';
import type { Session } from '@/lib/types';
import { t } from '@/lib/i18n';
import { Button } from '@/components/ui/button';

interface SidebarProps {
  sessions: Session[];
  activeId: string;
  // True once the ACP backend handshake succeeded.
  connected: boolean;
  onSelect: (id: string) => void;
  onNew: () => void;
}

function formatTime(ts: number): string {
  const diff = Date.now() - ts;
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return t('time.justNow');
  if (mins < 60) return t('time.minutesAgo', { n: mins });
  const hours = Math.floor(mins / 60);
  if (hours < 24) return t('time.hoursAgo', { n: hours });
  return t('time.daysAgo', { n: Math.floor(hours / 24) });
}

export function Sidebar({ sessions, activeId, connected, onSelect, onNew }: SidebarProps) {
  return (
    <aside className="flex w-[260px] shrink-0 flex-col border-r border-border bg-muted">
      <div className="flex h-12 shrink-0 items-center justify-between border-b border-border px-4">
        <span className="text-[15px] font-semibold">SrudAgent</span>
        <Button
          variant="outline"
          size="icon"
          className="h-6.5 w-6.5 rounded-md"
          onClick={onNew}
          title={t('sidebar.newChat')}
        >
          <Plus className="h-4 w-4" />
        </Button>
      </div>
      <nav className="flex flex-1 flex-col gap-0.5 overflow-y-auto p-2">
        {sessions.map((c) => (
          <button
            key={c.id}
            className={`flex cursor-pointer flex-col items-start gap-1 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-item-hover ${
              c.id === activeId ? 'bg-item-hover' : ''
            }`}
            onClick={() => onSelect(c.id)}
          >
            <span className="max-w-full truncate text-[13px]">{c.title}</span>
            <span className="text-[11px] text-muted-foreground">{formatTime(c.updatedAt)}</span>
          </button>
        ))}
      </nav>
      <div className="flex items-center gap-2 border-t border-border px-4 py-3 text-xs text-muted-foreground">
        <span className={`h-2 w-2 rounded-full ${connected ? 'bg-success' : 'bg-warning'}`} />
        <span>{t(connected ? 'sidebar.backendLive' : 'sidebar.backendMock')}</span>
      </div>
    </aside>
  );
}
