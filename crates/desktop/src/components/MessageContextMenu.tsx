import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Copy } from 'lucide-react';
import { toast } from 'sonner';
import { t } from '@/lib/i18n';

/** Where a message was right-clicked, and the text that click selected. */
export interface MessageMenuAnchor {
  x: number;
  y: number;
  /**
   * What Copy writes to the clipboard.
   *
   * This is the reader's selection when there is one inside the message, and the
   * message's own text otherwise. A menu that always copied the whole message
   * would quietly discard the sentence the reader had picked out.
   */
  text: string;
}

// Keeps the menu clear of the window edges once it has been measured.
const EDGE_MARGIN = 8;

/**
 * The menu a right-click opens over a user or model message.
 *
 * Rendered into `document.body` and positioned fixed: the transcript scrolls
 * underneath it, so leaving it in the list's own tree would drag the menu along
 * when the reader scrolled.
 */
export function MessageContextMenu({
  anchor,
  onClose,
}: {
  anchor: MessageMenuAnchor;
  onClose: () => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: anchor.x, top: anchor.y });

  // The pointer can be anywhere, including the last few pixels of the window, so
  // the menu is moved back inside the viewport rather than drawn off its edge.
  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;

    const { width, height } = menu.getBoundingClientRect();
    setPosition({
      left: Math.max(EDGE_MARGIN, Math.min(anchor.x, window.innerWidth - width - EDGE_MARGIN)),
      top: Math.max(EDGE_MARGIN, Math.min(anchor.y, window.innerHeight - height - EDGE_MARGIN)),
    });
  }, [anchor]);

  useEffect(() => {
    const onPointerDown = (event: Event) => {
      if (event.target instanceof Node && menuRef.current?.contains(event.target)) return;
      onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };

    // A fixed menu has no anchor that scrolls with the content, so any movement
    // of the transcript leaves it pointing at the wrong message.
    window.addEventListener('pointerdown', onPointerDown, true);
    window.addEventListener('keydown', onKeyDown);
    window.addEventListener('scroll', onClose, true);
    window.addEventListener('resize', onClose);
    return () => {
      window.removeEventListener('pointerdown', onPointerDown, true);
      window.removeEventListener('keydown', onKeyDown);
      window.removeEventListener('scroll', onClose, true);
      window.removeEventListener('resize', onClose);
    };
  }, [onClose]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(anchor.text);
    } catch {
      // Denied, or no clipboard available. There is nothing to recover to, so the
      // menu closes without claiming it worked.
      onClose();
      return;
    }
    toast.success(t('msg.copied'));
    onClose();
  };

  return createPortal(
    <div
      ref={menuRef}
      role="menu"
      aria-label={t('msg.actions')}
      style={position}
      className="fixed z-50 min-w-[160px] rounded-lg border border-border bg-background p-1 text-foreground shadow-[0_4px_16px_rgba(0,0,0,0.14)] dark:shadow-[0_4px_16px_rgba(0,0,0,0.5)]"
    >
      <button
        type="button"
        role="menuitem"
        onClick={() => void copy()}
        // No autofocus: the menu appears where the pointer already is, and focus
        // alone would paint this row as hovered before the pointer reached it.
        // `focus-visible` keeps the highlight for keyboard entry.
        className="flex w-full cursor-pointer select-none items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] outline-none transition-colors hover:bg-item-hover focus-visible:bg-item-hover"
      >
        <Copy className="h-3.5 w-3.5 text-muted-foreground" />
        {t('msg.copy')}
      </button>
    </div>,
    document.body,
  );
}
