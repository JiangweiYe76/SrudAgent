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
import { isAgentWorkspace, workspaceName } from '@/lib/workspace';
import { toast } from 'sonner';
import { DeleteSessionModal } from '@/components/DeleteSessionModal';
import { RenameSessionModal } from '@/components/RenameSessionModal';
import { SkillsModal } from '@/components/SkillsModal';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/components/ui/tooltip';
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
  // Starts a draft in the given directory instead of the one on screen.
  onNewInWorkspace: (cwd: string) => void;
  onRename: (id: string, title: string) => void;
  onFork: (id: string) => void;
  onDelete: (id: string) => void;
}

// Absolute time for the details a hover reveals: a relative label says how long
// ago, not when.
function formatDateTime(ts: number): string {
  return new Intl.DateTimeFormat(undefined, {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(ts);
}

interface WorkspaceGroup {
  /**
   * The directory to group by; empty for the default group, which gathers
   * agent-made workspaces and drafts that chose none.
   */
  path: string;
  label: string;
  sessions: Session[];
}

// Sessions grouped by the directory they work in.
//
// Every workspace the agent made counts as the default one: a session that chose
// no directory gets one of its own, and they all belong together under that one
// heading even though the directories differ. Groups are ordered by name and
// nothing else, with the default group first: adding a session to one of them,
// or working in it, must never move it.
function workspaceGroups(sessions: Session[]): WorkspaceGroup[] {
  const byPath = new Map<string, Session[]>();
  for (const session of sessions) {
    const picked = session.cwd && !isAgentWorkspace(session.cwd);
    const key = picked ? session.cwd : '';
    const members = byPath.get(key);
    if (members) members.push(session);
    else byPath.set(key, [session]);
  }
  return [...byPath.entries()]
    .map(([path, members]) => ({
      path,
      label: workspaceName(path),
      sessions: members,
    }))
    .sort(
      (a, b) =>
        Number(b.path === '') - Number(a.path === '') ||
        a.label.localeCompare(b.label),
    );
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
    <Tooltip>
      <TooltipTrigger asChild>
        <button
          className={`group flex cursor-pointer items-center gap-1 rounded-lg px-3 py-2 text-left transition-colors hover:bg-item-hover ${
            active ? 'bg-item-hover' : ''
          } ${deleting ? 'opacity-60' : ''}`}
          onClick={() => {
            if (!deleting) onSelect(session.id);
          }}
          aria-disabled={deleting || undefined}
        >
          {/* A single line keeps the list short; the details of when and where
              are one hover away. */}
          <span className="min-w-0 flex-1 truncate text-[13px]">
            {displayTitle(session)}
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
          {/* A session on its way out keeps its row until the agent agrees,
              dimmed and with a spinner where the actions were: firing delete
              again would stack a second request on the first. */}
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
                session.backendId && !streaming
                  ? () => onFork(session.id)
                  : null
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
      </TooltipTrigger>
      <TooltipContent side="right" align="start" className="max-w-[300px]">
        <div className="flex flex-col gap-1">
          <span className="text-[13px] font-medium">
            {displayTitle(session)}
          </span>
          <span className="break-all text-[11px] text-muted-foreground">
            {session.cwd || t('workspace.default')}
          </span>
          <span className="text-[11px] text-muted-foreground">
            {t('sidebar.created', { time: formatDateTime(session.createdAt) })}
          </span>
          <span className="text-[11px] text-muted-foreground">
            {t('sidebar.updated', { time: formatDateTime(session.updatedAt) })}
          </span>
        </div>
      </TooltipContent>
    </Tooltip>
  );
});

export const Sidebar = memo(function Sidebar({
  sessions,
  activeId,
  deletingIds,
  unreadIds,
  onSelect,
  onNew,
  onNewInWorkspace,
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
      <TooltipProvider delayDuration={400}>
        <nav className="flex flex-1 flex-col gap-2 overflow-y-auto p-2">
          {groups.map((group) => (
            <div key={group.path} className="flex flex-col gap-0.5">
              <div className="group flex items-center gap-1.5 rounded-lg px-3 py-2 text-[13px] text-muted-foreground transition-colors hover:bg-item-hover">
                <FolderOpen className="h-3.5 w-3.5 shrink-0" />
                <span className="min-w-0 flex-1 truncate">{group.label}</span>
                <button
                  title={t('sidebar.newSessionHere')}
                  aria-label={t('sidebar.newSessionHere')}
                  onClick={() => onNewInWorkspace(group.path)}
                  className="flex h-5 w-5 shrink-0 cursor-pointer items-center justify-center rounded-md opacity-0 transition-colors group-hover:opacity-100 hover:bg-item-hover hover:text-foreground focus-visible:opacity-100"
                >
                  <Plus className="h-3.5 w-3.5" />
                </button>
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
      </TooltipProvider>

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
});
