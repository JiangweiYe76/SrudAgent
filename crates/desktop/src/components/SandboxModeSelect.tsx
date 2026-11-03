import { useEffect, useState } from 'react';
import { Check, ChevronDown, ShieldAlert } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuItemDestructive,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import type { SandboxModeOptions } from '@/lib/types';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';

interface SandboxModeSelectProps {
  options: SandboxModeOptions | null | undefined;
  // Switching a mode is a session-level action. Selecting the mode already in
  // effect changes nothing.
  onSelect?: (mode: string) => void;
}

/**
 * The sandbox mode a session runs in, as a selector under the composer.
 *
 * Renders nothing when the agent advertised no mode. The rows are the modes the
 * agent listed; full access uses the destructive colour because of what it
 * means for the files this account can reach.
 */
export function SandboxModeSelect({
  options,
  onSelect,
}: SandboxModeSelectProps) {
  const [open, setOpen] = useState(false);

  // A session that goes away mid-open would otherwise leave the menu floating
  // over the next conversation.
  useEffect(() => {
    setOpen(false);
  }, [options]);

  if (!options || options.choices.length === 0) return null;

  const current =
    options.choices.find((choice) => choice.value === options.currentValue)
      ?.name ?? options.currentValue;
  const fullAccess = options.currentValue === 'full-access';

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger
        title={fullAccess ? t('sandbox.fullAccessHint') : current}
        className={cn(
          'flex cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-[13px] font-normal transition-colors',
          'hover:bg-item-hover focus-visible:bg-item-hover focus-visible:outline-none',
          fullAccess
            ? 'text-destructive hover:bg-destructive/10'
            : 'text-muted-foreground hover:text-foreground',
        )}
      >
        {fullAccess && <ShieldAlert className="h-3.5 w-3.5 shrink-0" />}
        <span className="truncate">{current}</span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" className="min-w-[200px]">
        {options.choices.map((choice) => {
          const selected = choice.value === options.currentValue;
          const entry = choice.value === 'full-access';
          const Item = entry ? DropdownMenuItemDestructive : DropdownMenuItem;
          return (
            <Item
              key={choice.value}
              onSelect={() => {
                onSelect?.(choice.value);
                setOpen(false);
              }}
            >
              <Check
                className={cn(
                  'h-3.5 w-3.5 shrink-0',
                  selected ? 'opacity-100' : 'opacity-0',
                )}
              />
              <span className="truncate">{choice.name}</span>
            </Item>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
