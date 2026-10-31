import { useEffect, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { UNAVAILABLE_GROUP } from '@/lib/acp';
import type { ModelOptions } from '@/lib/types';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';

interface ModelSelectProps {
  options: ModelOptions | null;
  // Set while a conversation is still arriving: a switch then would land in a
  // session the agent is in the middle of handing over.
  disabled?: boolean;
  onSelect: (model: string) => void;
}

// The model a session runs on, and the list of what it could run on instead.
//
// Renders nothing when the agent offered no choice: a host with one fixed model
// has nothing to pick, and an empty dropdown would claim otherwise.
export function ModelSelect({ options, disabled, onSelect }: ModelSelectProps) {
  const [open, setOpen] = useState(false);

  // A session that goes away mid-open — deleted, or switched away from — would
  // otherwise leave the menu floating over the next conversation.
  useEffect(() => {
    setOpen(false);
  }, [options]);

  if (!options || options.groups.length === 0) return null;

  // Nothing chosen: nobody has picked a default and this session has not been
  // given one either. The trigger says what to do rather than showing an empty
  // space where a model's name would be.
  const unchosen = options.currentValue === '';
  const current = unchosen ? t('models.choose') : labelOf(options);
  const stale = isStale(options);

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger
        disabled={disabled}
        title={current}
        className={cn(
          'flex max-w-[240px] cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-[13px] font-normal transition-colors',
          'hover:bg-item-hover focus-visible:bg-item-hover focus-visible:outline-none',
          'data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
          stale
            ? 'text-destructive'
            : unchosen
              ? 'text-accent hover:text-accent'
              : 'text-muted-foreground hover:text-foreground',
        )}
      >
        <span className="truncate">{current}</span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0" />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        align="end"
        className="max-h-[420px] min-w-[240px] overflow-y-auto"
      >
        {options.groups.map((group) => {
          const unavailable = group.id === UNAVAILABLE_GROUP;
          return (
            <div key={group.id || group.name}>
              {group.name && (
                <div className="px-2 pt-2 pb-1 text-[11px] font-medium tracking-wide text-muted-foreground uppercase">
                  {group.name}
                </div>
              )}
              {group.choices.map((choice) => (
                <DropdownMenuItem
                  key={choice.value}
                  // Picking what the session already runs on would change
                  // nothing, and a stale entry names a model that is gone.
                  disabled={
                    unavailable || choice.value === options.currentValue
                  }
                  onSelect={() => onSelect(choice.value)}
                  className={unavailable ? 'text-destructive' : undefined}
                >
                  <Check
                    className={cn(
                      'h-3.5 w-3.5 shrink-0',
                      choice.value === options.currentValue
                        ? 'opacity-100'
                        : 'opacity-0',
                    )}
                  />
                  <span className="truncate">
                    {unavailable
                      ? t('models.unavailable', { model: choice.name })
                      : choice.name}
                  </span>
                </DropdownMenuItem>
              ))}
            </div>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// What to show on the trigger: the chosen model's label, or its reference when
// nothing offers it — a provider deleted since the session chose it.
function labelOf(options: ModelOptions): string {
  for (const group of options.groups) {
    const found = group.choices.find(
      (choice) => choice.value === options.currentValue,
    );
    if (found) return found.name;
  }
  return options.currentValue;
}

// Whether the session's model is one no provider offers any more.
function isStale(options: ModelOptions): boolean {
  return options.groups.some(
    (group) =>
      group.id === UNAVAILABLE_GROUP &&
      group.choices.some((choice) => choice.value === options.currentValue),
  );
}
