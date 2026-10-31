import {
  useEffect,
  useState,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
} from 'react';
import type { LucideIcon } from 'lucide-react';
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuRoot,
  ContextMenuTrigger,
} from '@/components/ui/context-menu';

/** One action a context menu offers. */
export interface MenuItem {
  label: string;
  icon?: LucideIcon;
  disabled?: boolean;
  onSelect: () => void;
}

/**
 * A context menu over `children`, opened by right-clicking them.
 *
 * Radix owns the pointer anchoring and the dismissal: Escape, a pointer down
 * outside, and the close that follows a choice. The menu is fixed, so scroll
 * and resize close it as well — content moving underneath would leave it
 * pointing at the wrong line.
 */
export function ContextMenu({
  label,
  items,
  onOpen,
  children,
}: {
  label: string;
  items: MenuItem[];
  // Runs before the menu opens, while the trigger's selection is still current.
  onOpen?: (event: ReactMouseEvent<HTMLElement>) => void;
  children: ReactElement;
}) {
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!open) return;
    const close = () => setOpen(false);
    window.addEventListener('scroll', close, true);
    window.addEventListener('resize', close);
    return () => {
      window.removeEventListener('scroll', close, true);
      window.removeEventListener('resize', close);
    };
  }, [open]);

  return (
    <ContextMenuRoot open={open} onOpenChange={setOpen}>
      <ContextMenuTrigger asChild onContextMenu={onOpen}>
        {children}
      </ContextMenuTrigger>
      <ContextMenuContent aria-label={label}>
        {items.map(({ label: itemLabel, icon: Icon, disabled, onSelect }) => (
          <ContextMenuItem
            key={itemLabel}
            disabled={disabled}
            onSelect={onSelect}
          >
            {Icon && <Icon className="h-3.5 w-3.5 text-muted-foreground" />}
            {itemLabel}
          </ContextMenuItem>
        ))}
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}
