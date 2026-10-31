import { useEffect, useState } from 'react';
import { Check, ChevronDown } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import type { ThinkingOptions } from '@/lib/types';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';

interface ThinkingSelectProps {
  options: ThinkingOptions | null;
  // Set while a conversation is still arriving: a switch then would land in a
  // session the agent is in the middle of handing over.
  disabled?: boolean;
  onSelect: (level: string) => void;
}

// The thinking level a session sends, and the levels its model offers.
//
// Renders nothing when the selected model declares no thinking control: there
// is then nothing to choose, and an empty dropdown would claim otherwise. The
// values are canonical levels; the endpoint's own wire value is the model's
// business and never appears here.
export function ThinkingSelect({
  options,
  disabled,
  onSelect,
}: ThinkingSelectProps) {
  const [open, setOpen] = useState(false);

  // A session that goes away mid-open — deleted, or switched away from — would
  // otherwise leave the menu floating over the next conversation.
  useEffect(() => {
    setOpen(false);
  }, [options]);

  if (!options || options.choices.length === 0) return null;

  const current = labelOf(options);
  const unchosen = current === '';

  return (
    <DropdownMenu open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger
        disabled={disabled}
        title={t('thinking.title')}
        className={cn(
          'flex max-w-[160px] cursor-pointer items-center gap-1.5 rounded-md px-2 py-1 text-[13px] font-normal transition-colors',
          'hover:bg-item-hover focus-visible:bg-item-hover focus-visible:outline-none',
          'data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
          unchosen
            ? 'text-accent hover:text-accent'
            : 'text-muted-foreground hover:text-foreground',
        )}
      >
        <span className="truncate">
          {unchosen ? t('thinking.choose') : current}
        </span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-[150px]">
        {options.choices.map((choice) => (
          <DropdownMenuItem
            key={choice.value}
            // Picking what the session already sends would change nothing.
            disabled={choice.value === options.currentValue}
            onSelect={() => onSelect(choice.value)}
          >
            <Check
              className={cn(
                'h-3.5 w-3.5 shrink-0',
                choice.value === options.currentValue
                  ? 'opacity-100'
                  : 'opacity-0',
              )}
            />
            <span className="truncate">{choice.name}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// What to show on the trigger: the chosen level's label, or its value when the
// list does not hold it.
function labelOf(options: ThinkingOptions): string {
  return (
    options.choices.find((choice) => choice.value === options.currentValue)
      ?.name ?? options.currentValue
  );
}
