import type { Attachment } from '@/lib/types';
import { imageSrc } from '@/lib/images';
import { t } from '@/lib/i18n';
import { Modal, ModalContent } from '@/components/ui/modal';

/**
 * One image, opened over the app.
 *
 * The attachment is the whole input: the transcript and the composer both draw
 * images and open one on a click, and neither has anything to add to the
 * dialog beyond which attachment it hands over.
 *
 * The open image is the prop. `null` is what closes the dialog, so the caller
 * keeps no state of its own beyond the attachment.
 */
export function ImagePreview({
  image,
  onClose,
}: {
  image: Attachment | null;
  onClose: () => void;
}) {
  return (
    <Modal
      open={image !== null}
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <ModalContent
        title={t('chat.image.preview')}
        className="max-h-[90vh] max-w-[90vw]"
        overlayClassName="bg-black/70"
      >
        {image && (
          <img
            src={imageSrc(image)}
            alt={t('chat.image.preview')}
            className="max-h-[80vh] max-w-[80vw] object-contain"
          />
        )}
      </ModalContent>
    </Modal>
  );
}
