// The MCP servers dialog, opened from the sidebar beside Skills.
//
// The servers are configured here rather than per session: what a session runs
// with is whatever this list holds when the session is created. The panel is
// the same one the settings dialog would show, so there is a single surface to
// keep right.
import { Modal, ModalContent } from '@/components/ui/modal';
import { McpPanel } from '@/components/McpPanel';
import { t } from '@/lib/i18n';

interface McpModalProps {
  open: boolean;
  onClose: () => void;
}

export function McpModal({ open, onClose }: McpModalProps) {
  return (
    <Modal open={open} onOpenChange={(next) => !next && onClose()}>
      <ModalContent title={t('mcp.title')} className="h-[600px] w-[880px]">
        <div className="flex min-h-0 flex-1 flex-col p-4">
          <McpPanel />
        </div>
      </ModalContent>
    </Modal>
  );
}
