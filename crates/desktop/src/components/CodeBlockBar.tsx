// The titled bar that sits on top of a code or diagram block.
//
// Shared rather than duplicated: a code block and a diagram are different things
// underneath, but they are the same shape to a reader — a header with a label on
// the left and controls on the right — and two copies of that markup would drift
// apart the first time one of them gained a control.
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Check, Copy } from 'lucide-react';
import { t } from '@/lib/i18n';

/** A button that copies text and says so for a moment afterwards. */
export function CopyCode({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  // The timer is held so it can be replaced or cancelled rather than left to
  // fire: two clicks inside the window would otherwise leave two timers running,
  // and the first would clear the confirmation the second click had just set.
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Denied, or no clipboard available — the WebView may be unfocused. Nothing
      // to recover to, so the button simply does not claim it worked.
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };

  // Leaving the timer to outlive the button would set state on a component that
  // is no longer on screen.
  useEffect(() => () => clearTimeout(timer.current), []);

  return (
    <button
      // Always drawn rather than revealed on hover: the control is the reason to
      // title the block at all, and hiding it leaves the bar looking inert.
      className="flex cursor-pointer items-center rounded p-1 text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
      onClick={copy}
      title={copied ? t('code.copied') : t('code.copy')}
    >
      {copied ? <Check className="h-3.5 w-3.5" /> : <Copy className="h-3.5 w-3.5" />}
    </button>
  );
}

/**
 * A bar with a label on the left and controls on the right.
 *
 * `children` are the block's own controls, placed before the copy button: copy is
 * the common action, so it keeps the far corner to itself rather than competing
 * with whatever else the block offers.
 */
export function CodeBlockBar({
  label,
  text,
  children,
}: {
  label: string;
  text: string;
  children?: ReactNode;
}) {
  return (
    <div className="code-block-bar">
      <span className="code-block-lang">{label}</span>
      <div className="flex items-center gap-0.5">
        {children}
        <CopyCode text={text} />
      </div>
    </div>
  );
}
