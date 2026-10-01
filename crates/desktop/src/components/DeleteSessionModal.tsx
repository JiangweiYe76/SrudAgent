import { t } from '@/lib/i18n';
import { Button } from '@/components/ui/button';
import { Modal, ModalContent } from '@/components/ui/modal';

interface DeleteSessionModalProps {
  open: boolean;
  // The session's display name, for naming what is about to go.
  title: string;
  // How many turns it holds, so the wording can say whether a conversation is
  // being discarded or just an empty session.
  turns: number;
  onClose: () => void;
  onConfirm: () => void;
}

export function DeleteSessionModal({
  open,
  title,
  turns,
  onClose,
  onConfirm,
}: DeleteSessionModalProps) {
  return (
    <Modal open={open} onOpenChange={(next) => !next && onClose()}>
      <ModalContent title={t('sidebar.deleteTitle')} className="w-[420px]">
        <div className="flex flex-col gap-4 p-5">
          <p className="m-0 text-sm text-muted-foreground">
            {turns > 0
              ? t('sidebar.deleteWarningWithTurns', { title, n: turns })
              : t('sidebar.deleteWarning', { title })}
          </p>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="destructive" onClick={onConfirm}>
              {t('sidebar.deleteConfirm')}
            </Button>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
}
