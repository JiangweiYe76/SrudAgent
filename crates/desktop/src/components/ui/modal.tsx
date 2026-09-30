import * as React from 'react';
import * as DialogPrimitive from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import { t } from '@/lib/i18n';
import { cn } from '@/lib/utils';

export const Modal = DialogPrimitive.Root;
export const ModalTrigger = DialogPrimitive.Trigger;
export const ModalClose = DialogPrimitive.Close;

export function ModalContent({
  className,
  children,
  title,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & { title: string }) {
  return (
    <DialogPrimitive.Portal>
      <DialogPrimitive.Overlay className="fixed inset-0 z-50 bg-black/40" />
      <DialogPrimitive.Content
        aria-describedby={undefined}
        className={cn(
          'fixed top-1/2 left-1/2 z-50 flex -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-xl border border-border bg-background shadow-[0_12px_40px_rgba(0,0,0,0.18)] outline-none',
          className,
        )}
        {...props}
      >
        <div className="flex h-12 shrink-0 items-center justify-between border-b border-border px-4">
          <DialogPrimitive.Title className="text-sm font-semibold">{title}</DialogPrimitive.Title>
          <DialogPrimitive.Close
            title={t('common.close')}
            aria-label={t('common.close')}
            className="flex cursor-pointer items-center rounded-md p-1 text-muted-foreground transition-colors hover:bg-item-hover hover:text-foreground"
          >
            <X className="h-4 w-4" />
          </DialogPrimitive.Close>
        </div>

        <div className="flex min-h-0 flex-1 flex-col">{children}</div>
      </DialogPrimitive.Content>
    </DialogPrimitive.Portal>
  );
}
