// A labelled control, the shape every form in the app uses.
//
// By default the `<label>` wraps its control rather than pointing at it by id,
// which is what lets a click anywhere on the label — including the caption —
// land in the field without either end needing a generated id.
import type { ReactNode } from 'react';
import { cn } from '@/lib/utils';

interface FieldProps {
  label: string;
  // A line under the control: what the field means, or what happens if it is
  // left alone. Separate from the control so it can be a sentence.
  hint?: ReactNode;
  children: ReactNode;
  // A label may only take one labelable descendant, and it forwards hover and
  // clicks to that descendant. A field holding a set of buttons therefore has
  // to render in a plain div, where the caption is no longer a label at all.
  as?: 'label' | 'div';
}

export function Field({ label, hint, children, as = 'label' }: FieldProps) {
  const Tag = as;
  return (
    <Tag className="flex min-w-0 flex-1 flex-col gap-1">
      <span className="text-xs text-muted-foreground">{label}</span>
      {children}
      {hint}
    </Tag>
  );
}

/** The plain text control that fills a [`Field`]. */
export function TextInput({
  value,
  onChange,
  placeholder,
  disabled,
  autoFocus,
  className,
}: {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  autoFocus?: boolean;
  className?: string;
}) {
  return (
    <input
      type="text"
      value={value}
      placeholder={placeholder}
      disabled={disabled}
      autoFocus={autoFocus}
      spellCheck={false}
      onChange={(event) => onChange(event.target.value)}
      className={cn(
        'w-full rounded-md border border-border bg-background px-2 py-1.5 text-[13px] outline-none focus-visible:border-accent disabled:opacity-60',
        className,
      )}
    />
  );
}

export interface ToggleOption<T extends string> {
  value: T;
  label: string;
  icon?: ReactNode;
}

/**
 * A row of independent on/off choices.
 *
 * A checkbox group rather than a radio group: the choices are not exclusive, so
 * pressing one must not clear the others. Each option is its own button with
 * `aria-pressed`, which is what a screen reader reads and what a test taps.
 */
export function ToggleGroup<T extends string>({
  options,
  value,
  onChange,
}: {
  options: ToggleOption<T>[];
  value: T[];
  onChange: (value: T[]) => void;
}) {
  return (
    <div className="flex flex-wrap gap-1.5">
      {options.map((option) => {
        const on = value.includes(option.value);
        return (
          <button
            key={option.value}
            type="button"
            aria-pressed={on}
            onClick={() =>
              onChange(
                on ? value.filter((v) => v !== option.value) : [...value, option.value],
              )
            }
            className={cn(
              'inline-flex cursor-pointer items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/40',
              on
                ? 'border-accent bg-accent/10 text-foreground'
                : 'border-border bg-transparent text-muted-foreground hover:bg-item-hover',
            )}
          >
            {option.icon}
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
