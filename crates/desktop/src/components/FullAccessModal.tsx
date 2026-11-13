import { memo } from 'react';
import { t } from '@/lib/i18n';
import { Button } from '@/components/ui/button';
import { Modal, ModalContent } from '@/components/ui/modal';

interface FullAccessModalProps {
  open: boolean;
  onClose: () => void;
  onConfirm: () => void;
}

/**
 * The confirmation before a session leaves its OS boundary.
 *
 * The three points are what a person needs to decide: where the mode applies,
 * what it allows, and how to leave it.
 */
export const FullAccessModal = memo(function FullAccessModal({
  open,
  onClose,
  onConfirm,
}: FullAccessModalProps) {
  return (
    <Modal open={open} onOpenChange={(next) => !next && onClose()}>
      <ModalContent title={t('sandbox.fullAccessTitle')} className="w-[460px]">
        <div className="flex flex-col gap-4 p-5">
          <ul className="m-0 flex list-disc flex-col gap-2 pl-5 text-sm text-muted-foreground">
            <li>{t('sandbox.fullAccessScope')}</li>
            <li>{t('sandbox.fullAccessConsequence')}</li>
            <li>{t('sandbox.fullAccessExit')}</li>
          </ul>
          <div className="flex justify-end gap-2">
            <Button variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button variant="destructive" onClick={onConfirm}>
              {t('sandbox.enableFullAccess')}
            </Button>
          </div>
        </div>
      </ModalContent>
    </Modal>
  );
});
