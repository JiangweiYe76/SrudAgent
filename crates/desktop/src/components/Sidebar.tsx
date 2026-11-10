import { memo, useMemo, useState } from 'react';
import {
  FolderOpen,
  GitFork,
  Loader2,
  MoreHorizontal,
  Pencil,
  Plus,
  Sparkles,
  Trash2,
} from 'lucide-react';
import type { Session } from '@/lib/types';
import { t } from '@/lib/i18n';
import { deleteWarning, displayTitle, isBusy } from '@/lib/sessionStore';
import { workspaceName } from '@/lib/workspace';
import { toast } from 'sonner';
import { DeleteSessionModal } from '@/components/DeleteSessionModal';
import { RenameSessionModal } from '@/components/RenameSessionModal';
import { SkillsModal } from '@/components/SkillsModal';
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
  // Sessions whose delete is in flight. Their rows stay put — a refused delete
  // must lose nothing — so they show this instead of the actions.
  deletingIds: string[];
  // Sessions whose last turn ended off screen and have not been opened since.
  unreadIds: string[];
  onSelect: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, title: string) => void;
  onFork: (id: string) => void;
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

interface WorkspaceGroup {
  /** The directory itself; empty for a draft that has not chosen one. */
  path: string;
  label: string;
  sessions: Session[];
}

// Sessions grouped by the directory they work in.
//
// The agent names a workspace it creates after the session it belongs to, which
// is how one of those is told apart from a directory the user picked. Groups
// keep the order their first session arrived in, so a conversation stays where
// the list put it.
function workspaceGroups(sessions: Session[]): WorkspaceGroup[] {
  const byPath = new Map<string, Session[]>();
  for (const session of sessions) {
    const members = byPath.get(session.cwd);
    if (members) members.push(session);
    else byPath.set(session.cwd, [session]);
  }
  return [...byPath.entries()].map(([path, members]) => ({
    path,
    label: workspaceLabel(path, members),
    sessions: members,
  }));
}

function workspaceLabel(path: string, members: Session[]): string {
  if (!path) return t('workspace.default');
  const name = workspaceName(path);
  return members.some((s) => s.backendId === name)
    ? t('workspace.default')
    : name;
}

// The per-session actions, behind one trigger so the row stays quiet. Sessions
// will collect more actions than fit, so this is a menu rather than an icon row.
function SessionActions({
  onRename,
  onFork,
  onDelete,
}: {
  onRename: () => void;
  // `null` when there is nothing to fork: a draft has no history yet, and a
  // session still replying has no settled point to fork at.
  onFork: (() => void) | null;
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
        {onFork && (
          <DropdownMenuItem onSelect={onFork}>
            <GitFork className="h-3.5 w-3.5 text-muted-foreground" />
            {t('sidebar.forkTitle')}
          </DropdownMenuItem>
        )}
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

interface SessionRowProps {
  session: Session;
  active: boolean;
  deleting: boolean;
  streaming: boolean;
  unread: boolean;
  onSelect: (id: string) => void;
  onFork: (id: string) => void;
  onRequestRename: (id: string) => void;
  onRequestDelete: (id: string) => void;
}

// The store replaces only the session a write names, so rows for other sessions
// keep their props and memo leaves them alone.
const SessionRow = memo(function SessionRow({
  session,
  active,
  deleting,
  streaming,
  unread,
  onSelect,
  onFork,
  onRequestRename,
  onRequestDelete,
}: SessionRowProps) {
  return (
    <button
      className={`group flex cursor-pointer items-center gap-1 rounded-lg px-3 py-2.5 text-left transition-colors hover:bg-item-hover ${
        active ? 'bg-item-hover' : ''
      } ${deleting ? 'opacity-60' : ''}`}
      onClick={() => {
        if (!deleting) onSelect(session.id);
      }}
      aria-disabled={deleting || undefined}
    >
      {/* The title and its timestamp stack; the actions centre against
          both lines rather than sitting on the title alone. */}
      <span className="flex min-w-0 flex-1 flex-col items-start gap-1">
        <span className="w-full truncate text-[13px]">
          {displayTitle(session)}
        </span>
        <span className="text-[11px] text-muted-foreground">
          {formatTime(session.updatedAt)}
        </span>
      </span>
      {(streaming || unread) && (
        // Deliberately no `role="status"`: that is the deleting
        // spinner's contract, and tests read it as such. The state
        // travels in the title instead.
        <span
          title={streaming ? t('sidebar.streaming') : t('sidebar.unread')}
          className={`h-2 w-2 shrink-0 rounded-full ${
            streaming ? 'animate-presence-dot bg-accent' : 'bg-success'
          }`}
        />
      )}
      {/* A session on its way out keeps its row until the agent agrees, dimmed
          and with a spinner where the actions were: firing delete again would
          stack a second request on the first. */}
      {deleting ? (
        <span
          role="status"
          aria-label={t('sidebar.deleting')}
          title={t('sidebar.deleting')}
          className="flex shrink-0 items-center justify-center p-1"
        >
          <Loader2 className="h-3.5 w-3.5 animate-spin text-muted-foreground" />
        </span>
      ) : (
        <SessionActions
          onRename={() => onRequestRename(session.id)}
          onFork={
            session.backendId && !streaming ? () => onFork(session.id) : null
          }
          onDelete={() => {
            if (isBusy(session)) {
              toast.error(t('sidebar.deleteBusy'));
              return;
            }
            onRequestDelete(session.id);
          }}
        />
      )}
    </button>
  );
});

export function Sidebar({
  sessions,
  activeId,
  deletingIds,
  unreadIds,
  onSelect,
  onNew,
  onRename,
  onFork,
  onDelete,
}: SidebarProps) {
  // Which dialog is open, if any. Renaming and deleting are dialogs rather than
  // inline fields so the row layout never shifts.
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [skillsOpen, setSkillsOpen] = useState(false);

  // Read from `sessions` on every render, so a session closed while a dialog is
  // open resolves to undefined instead of acting on one that is gone.
  const renameTarget = renamingId
    ? sessions.find((s) => s.id === renamingId)
    : undefined;
  const deleteTarget = deletingId
    ? sessions.find((s) => s.id === deletingId)
    : undefined;
  const deleteTurns = deleteWarning(deleteTarget);
  // A draft may already point at a directory, so its project skills are listed
  // before the first message is sent.
  const skillsCwd = sessions.find((s) => s.id === activeId)?.cwd ?? '';
  const deletingSet = useMemo(() => new Set(deletingIds), [deletingIds]);
  const unreadSet = useMemo(() => new Set(unreadIds), [unreadIds]);
  const groups = useMemo(() => workspaceGroups(sessions), [sessions]);

  return (
    <aside className="flex w-[260px] shrink-0 flex-col border-r border-border bg-muted">
      <div className="flex h-12 shrink-0 items-center justify-between border-b border-border px-4">
        <span className="text-[15px] font-semibold">SrudAgent</span>
        {/* The size and shape come from `size="icon"`, like every other icon button
            in the shell. Overriding them made this one 26px square against the rest at
            32px, with a square corner against their circles. */}
        <Button
          variant="ghost"
          size="icon"
          onClick={onNew}
          title={t('sidebar.newChat')}
          aria-label={t('sidebar.newChat')}
        >
          <Plus className="h-4 w-4" />
        </Button>
      </div>
      <div className="shrink-0 border-b border-border p-2">
        <button
          className="flex w-full cursor-pointer items-center gap-2 rounded-lg px-3 py-2 text-left text-[13px] text-muted-foreground transition-colors hover:bg-item-hover hover:text-foreground"
          onClick={() => setSkillsOpen(true)}
          aria-haspopup="dialog"
        >
          <Sparkles className="h-4 w-4 shrink-0" />
          {t('skills.open')}
        </button>
      </div>
      <nav className="flex flex-1 flex-col gap-2 overflow-y-auto p-2">
        {groups.map((group) => (
          <div key={group.path} className="flex flex-col gap-0.5">
            <div
              title={group.path || undefined}
              className="flex items-center gap-1.5 px-3 pt-1 pb-0.5 text-[11px] font-medium tracking-wide text-muted-foreground uppercase"
            >
              <FolderOpen className="h-3 w-3 shrink-0" />
              <span className="truncate">{group.label}</span>
            </div>
            {group.sessions.map((s) => {
              const deleting = deletingSet.has(s.id);
              // Streaming outranks unread: a session that is still replying has
              // nothing finished to catch up on yet.
              const streaming = !deleting && isBusy(s);
              const unread = !deleting && !streaming && unreadSet.has(s.id);
              return (
                <SessionRow
                  key={s.id}
                  session={s}
                  active={s.id === activeId}
                  deleting={deleting}
                  streaming={streaming}
                  unread={unread}
                  onSelect={onSelect}
                  onFork={onFork}
                  onRequestRename={setRenamingId}
                  onRequestDelete={setDeletingId}
                />
              );
            })}
          </div>
        ))}
      </nav>

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
      <SkillsModal
        open={skillsOpen}
        cwd={skillsCwd}
        onClose={() => setSkillsOpen(false)}
      />
    </aside>
  );
}
