import { useState } from 'react';
import { ChevronDown, FolderOpen } from 'lucide-react';
import { toast } from 'sonner';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { t } from '@/lib/i18n';
import { cn } from '@/lib/utils';
import {
  isAgentWorkspace,
  pickWorkspace,
  workspaceName,
} from '@/lib/workspace';

interface WorkspaceSelectProps {
  /** The session's directory, or empty for the agent's own workspace. */
  cwd: string;
  /** Whether the session is still a draft that can be pointed elsewhere. */
  editable: boolean;
  disabled?: boolean;
  onChange: (path: string) => void;
}

// The directory a session works in.
//
// A draft opens the platform's directory picker; a session the agent has
// already created shows the directory as a read-only label, because ACP fixes
// it at setup.
export function WorkspaceSelect({
  cwd,
  editable,
  disabled,
  onChange,
}: WorkspaceSelectProps) {
  const [open, setOpen] = useState(false);
  const name =
    !cwd || isAgentWorkspace(cwd) ? t('workspace.default') : workspaceName(cwd);
  const title = cwd || t('workspace.defaultHint');

  const choose = async () => {
    try {
      const chosen = await pickWorkspace();
      if (chosen) onChange(chosen);
    } catch {
      toast.error(t('workspace.pickFailed'));
    }
  };

  if (!editable) {
    return (
      <span
        title={title}
        className="flex max-w-[200px] items-center gap-1.5 rounded-md px-2 py-1 font-normal text-muted-foreground"
      >
        <FolderOpen className="h-3.5 w-3.5 shrink-0" />
        <span className="truncate">{name}</span>
      </span>
    );
  }

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger
        disabled={disabled}
        title={title}
        className={cn(
          'flex max-w-[200px] cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 font-normal transition-colors',
          'hover:bg-item-hover focus-visible:bg-item-hover focus-visible:outline-none',
          'data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
          cwd
            ? 'text-muted-foreground hover:text-foreground'
            : 'text-accent hover:text-accent',
        )}
      >
        <FolderOpen className="h-3.5 w-3.5 shrink-0" />
        <span className="truncate">{name}</span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[220px]">
        <DropdownMenuItem
          onSelect={() => {
            setOpen(false);
            void choose();
          }}
        >
          <FolderOpen className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          {t('workspace.choose')}
        </DropdownMenuItem>
        {/* Choosing the default is only a change when something else is
            chosen, so there is no row for a draft already on it. */}
        {cwd && (
          <>
            <DropdownMenuSeparator />
            <DropdownMenuItem onSelect={() => onChange('')}>
              {t('workspace.useDefault')}
            </DropdownMenuItem>
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
