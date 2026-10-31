// A form control that picks one value from a short list.
//
// Built on the same menu the rest of the app uses rather than the system's
// native list: a native control cannot be themed, and its popup is the only
// piece of the window that would not match. The trigger is styled like a text
// field, because that is what it replaces in a form.
import { Check, ChevronDown } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { cn } from '@/lib/utils';

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface SelectProps {
  value: string;
  onChange: (value: string) => void;
  options: SelectOption[];
  // What the trigger says when `value` names nothing in the list.
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  // The field a screen reader announces; a surrounding label does not reach a
  // button the way it reaches a text input.
  ariaLabel?: string;
}

export function Select({
  value,
  onChange,
  options,
  placeholder,
  disabled,
  className,
  ariaLabel,
}: SelectProps) {
  const current = options.find((option) => option.value === value);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        disabled={disabled}
        aria-label={ariaLabel}
        className={cn(
          'flex w-full cursor-pointer items-center justify-between gap-2 rounded-md border border-border bg-background px-2 py-1.5 text-[13px] text-left transition-colors outline-none',
          'hover:bg-item-hover focus-visible:border-accent data-[disabled]:pointer-events-none data-[disabled]:opacity-60',
          className,
        )}
      >
        <span className={cn('truncate', !current && 'text-muted-foreground')}>
          {current?.label ?? placeholder ?? value}
        </span>
        <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="min-w-[140px]">
        {options.map((option) => (
          <DropdownMenuItem
            key={option.value}
            // Picking what is already chosen would change nothing.
            disabled={option.disabled || option.value === value}
            onSelect={() => onChange(option.value)}
          >
            <Check
              className={cn(
                'h-3.5 w-3.5 shrink-0',
                option.value === value ? 'opacity-100' : 'opacity-0',
              )}
            />
            <span className="truncate">{option.label}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
