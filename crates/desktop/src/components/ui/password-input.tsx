// A field that conceals what it holds until asked to show it.
//
// A credential is read back from the agent as a mask and never as itself, so the
// only place the real value is ever on screen is here — and only while someone
// has deliberately asked to look at it. The toggle is a button rather than a
// hover trick, so it works from the keyboard and on a touch screen.
import { useState } from 'react';
import { Eye, EyeOff } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { cn } from '@/lib/utils';
import { t } from '@/lib/i18n';

interface PasswordInputProps {
  value: string;
  onChange: (value: string) => void;
  placeholder?: string;
  disabled?: boolean;
  autoFocus?: boolean;
  className?: string;
}

export function PasswordInput({
  value,
  onChange,
  placeholder,
  disabled,
  autoFocus,
  className,
}: PasswordInputProps) {
  const [visible, setVisible] = useState(false);
  // The label names what the button does next, which is what a screen reader
  // should say — "Show" while it is hidden, "Hide" once it is not.
  const label = t(visible ? 'common.conceal' : 'common.reveal');

  return (
    <div className={cn('relative', className)}>
      <input
        // `password` rather than `text` with a mask drawn on: the browser is
        // what keeps it out of a screenshot, a screen reader and the clipboard.
        type={visible ? 'text' : 'password'}
        value={value}
        placeholder={placeholder}
        disabled={disabled}
        autoFocus={autoFocus}
        spellCheck={false}
        autoComplete="off"
        onChange={(event) => onChange(event.target.value)}
        // Room on the right for the toggle, so long keys do not run under it.
        className="w-full rounded-md border border-border bg-background py-1.5 pr-8 pl-2 text-[13px] outline-none focus-visible:border-accent disabled:opacity-60"
      />
      <Button
        type="button"
        variant="ghost"
        size="icon"
        title={label}
        aria-label={label}
        aria-pressed={visible}
        disabled={disabled}
        onClick={() => setVisible((current) => !current)}
        className="absolute top-1/2 right-0.5 h-6 w-6 -translate-y-1/2"
      >
        {visible ? <EyeOff className="h-3.5 w-3.5" /> : <Eye className="h-3.5 w-3.5" />}
      </Button>
    </div>
  );
}
