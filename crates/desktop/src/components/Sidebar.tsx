import { useState } from 'react';
import { MoreHorizontal, Pencil, Plus, Trash2 } from 'lucide-react';
import type { Session } from '@/lib/types';
import { t } from '@/lib/i18n';
import { deleteWarning, displayTitle } from '@/lib/sessionStore';
import { DeleteSessionModal } from '@/components/DeleteSessionModal';
import { RenameSessionModal } from '@/components/RenameSessionModal';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuItemDestructive,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

interface SidebarProps {
  sessions: Session[];
  activeId: string;
  // True once the ACP backend handshake succeeded.
  connected: boolean;
  onSelect: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
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

// The per-session actions, behind one trigger so the row stays quiet. Sessions
// will collect more actions than fit, so this is a menu rather than an icon row.
function SessionActions({
  onRename,
  onDelete,
}: {
  onRename: () => void;
  onDelete: () => void;
}) {
  return (
    // The menu must not bubble to the row, or opening it would also select the
    // session behind it.
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <span
          role="button"
          tabIndex={-1}
          aria-label={t('sidebar.actions')}
          title={t('sidebar.actions')}
          className="flex shrink-0 cursor-pointer items-center justify-center rounded p-1 text-muted-foreground opacity-0 transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100 data-[state=open]:opacity-100"
          onClick={(e) => e.stopPropagation()}
        >
          <MoreHorizontal className="h-3.5 w-3.5" />
        </span>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem onSelect={onRename}>
          <Pencil className="h-3.5 w-3.5 text-muted-foreground" />
          {t('sidebar.renameTitle')}
        </DropdownMenuItem>
        {/* Delete is separated and coloured: it cannot be undone. */}
        <DropdownMenuSeparator />
        <DropdownMenuItemDestructive onSelect={onDelete}>
          <Trash2 className="h-3.5 w-3.5" />
          {t('sidebar.deleteTitle')}
        </DropdownMenuItemDestructive>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function Sidebar({
  sessions,
  activeId,
  connected,
  onSelect,
  onNew,
  onRename,
  onDelete,
}: SidebarProps) {
  // Which dialog is open, if any. Renaming and deleting are dialogs rather than
  // inline fields so the row layout never shifts.
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);

  // Read from `sessions` on every render, so a session closed while a dialog is
  // open resolves to undefined instead of acting on one that is gone.
  const renameTarget = renamingId ? sessions.find((s) => s.id === renamingId) : undefined;
  const deleteTarget = deletingId ? sessions.find((s) => s.id === deletingId) : undefined;
  const deleteTurns = deleteWarning(deleteTarget);

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
        {sessions.map((s) => (
          <button
            key={s.id}
            className={`group flex cursor-pointer items-center gap-1 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-item-hover ${
              s.id === activeId ? 'bg-item-hover' : ''
            }`}
            onClick={() => onSelect(s.id)}
          >
            {/* The title and its timestamp stack; the actions centre against
                both lines rather than sitting on the title alone. */}
            <span className="flex min-w-0 flex-1 flex-col items-start gap-1">
              <span className="w-full truncate text-[13px]">{displayTitle(s)}</span>
              <span className="text-[11px] text-muted-foreground">{formatTime(s.updatedAt)}</span>
            </span>
            <SessionActions
              onRename={() => setRenamingId(s.id)}
              onDelete={() => setDeletingId(s.id)}
            />
          </button>
        ))}
      </nav>
      <div className="flex items-center gap-2 border-t border-border px-4 py-3 text-xs text-muted-foreground">
        <span className={`h-2 w-2 rounded-full ${connected ? 'bg-success' : 'bg-warning'}`} />
        <span>{t(connected ? 'sidebar.backendLive' : 'sidebar.backendMock')}</span>
      </div>

      <RenameSessionModal
        open={renameTarget !== undefined}
        title={renameTarget?.title ?? null}
        onClose={() => setRenamingId(null)}
        onRename={(title) => {
          if (renameTarget) onRename(renameTarget.id, title);
        }}
      />
      <DeleteSessionModal
        open={deleteTarget !== undefined}
        title={displayTitle(deleteTarget)}
        turns={deleteTurns}
        onClose={() => setDeletingId(null)}
        onConfirm={() => {
          if (deleteTarget) onDelete(deleteTarget.id);
          setDeletingId(null);
        }}
      />
    </aside>
  );
}
