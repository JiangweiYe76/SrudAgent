import {
  useRef,
  type MouseEvent as ReactMouseEvent,
  type ReactElement,
} from 'react';
import { Copy } from 'lucide-react';
import { toast } from 'sonner';
import { ContextMenu } from '@/components/ContextMenu';
import { sourceOfSelection } from '@/lib/markdownSource';
import { t } from '@/lib/i18n';

/**
 * The menu a right-click opens over a user or model message.
 *
 * `text` is the message's own text, used when nothing is selected inside it.
 */
export function MessageContextMenu({
  text,
  children,
}: {
  text: string;
  children: ReactElement;
}) {
  // What Copy writes: the reader's selection when one is inside the message,
  // and the message's own text otherwise.
  const copyText = useRef(text);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(copyText.current);
    } catch {
      // Denied, or no clipboard available. There is nothing to recover to, so the
      // menu closes without claiming it worked.
      return;
    }
    toast.success(t('msg.copied'));
  };

  const rememberSelection = (event: ReactMouseEvent<HTMLElement>) => {
    const selection = window.getSelection();
    const selectionAnchor = selection?.anchorNode;
    // The selection is read back as markdown, so a formula comes out as the
    // TeX the model wrote rather than KaTeX's output.
    const selected =
      selection &&
      selectionAnchor &&
      event.currentTarget.contains(selectionAnchor)
        ? (sourceOfSelection(selection, event.currentTarget, text) ??
          selection.toString())
        : '';
    copyText.current = selected || text;
  };

  return (
    <ContextMenu
      label={t('msg.actions')}
      onOpen={rememberSelection}
      items={[
        { label: t('msg.copy'), icon: Copy, onSelect: () => void copy() },
      ]}
    >
      {children}
    </ContextMenu>
  );
}
