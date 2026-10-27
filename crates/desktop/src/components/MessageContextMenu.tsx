import { Copy } from 'lucide-react';
import { toast } from 'sonner';
import { ContextMenu } from '@/components/ContextMenu';
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

/**
 * The menu a right-click opens over a user or model message.
 */
export function MessageContextMenu({
  anchor,
  onClose,
}: {
  anchor: MessageMenuAnchor;
  onClose: () => void;
}) {
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(anchor.text);
    } catch {
      // Denied, or no clipboard available. There is nothing to recover to, so the
      // menu closes without claiming it worked.
      return;
    }
    toast.success(t('msg.copied'));
  };

  return (
    <ContextMenu
      anchor={anchor}
      label={t('msg.actions')}
      onClose={onClose}
      items={[{ label: t('msg.copy'), icon: Copy, onSelect: () => void copy() }]}
    />
  );
}
