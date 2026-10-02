import { useEffect, useRef, useState } from 'react';
import { t } from '@/lib/i18n';
import { Button } from '@/components/ui/button';
import { Modal, ModalContent } from '@/components/ui/modal';

interface RenameSessionModalProps {
  open: boolean;
  // The title to seed the field with, or null for an unnamed session.
  title: string | null;
  onClose: () => void;
  onRename: (title: string) => void;
}

export function RenameSessionModal({ open, title, onClose, onRename }: RenameSessionModalProps) {
  const [draft, setDraft] = useState(title ?? '');
  const inputRef = useRef<HTMLInputElement>(null);

  // Reopening for another session must show that session's title, not the last
  // one edited, so the field is reseeded whenever the dialog opens.
  useEffect(() => {
    if (open) setDraft(title ?? '');
  }, [open, title]);

  useEffect(() => {
    if (open) inputRef.current?.select();
  }, [open]);

  const commit = () => {
    onRename(draft.trim());
    onClose();
  };

  return (
    <Modal open={open} onOpenChange={(next) => !next && onClose()}>
      <ModalContent title={t('sidebar.renameTitle')} className="w-[400px]">
        <form
          className="flex flex-col gap-4 p-5"
          onSubmit={(e) => {
            e.preventDefault();
            commit();
          }}
        >
          <input
            ref={inputRef}
            className="w-full rounded-lg border border-border bg-background px-3 py-2 text-sm outline-none focus:border-accent"
            value={draft}
            placeholder={t('sidebar.renamePlaceholder')}
            onChange={(e) => setDraft(e.target.value)}
          />
          <div className="flex justify-end gap-2">
            <Button type="button" variant="ghost" onClick={onClose}>
              {t('common.cancel')}
            </Button>
            <Button type="submit">{t('sidebar.renameSave')}</Button>
          </div>
        </form>
      </ModalContent>
    </Modal>
  );
}
