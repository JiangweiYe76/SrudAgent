import * as React from 'react';
import * as ContextMenuPrimitive from '@radix-ui/react-context-menu';

export const ContextMenuRoot = ContextMenuPrimitive.Root;
export const ContextMenuTrigger = ContextMenuPrimitive.Trigger;

export function ContextMenuContent({
  className,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Content>) {
  return (
    <ContextMenuPrimitive.Portal>
      <ContextMenuPrimitive.Content
        className={[
          'z-50 min-w-[160px] rounded-lg border border-border bg-background p-1 text-foreground shadow-[0_4px_16px_rgba(0,0,0,0.14)] outline-none dark:shadow-[0_4px_16px_rgba(0,0,0,0.5)]',
          className,
        ]
          .filter(Boolean)
          .join(' ')}
        {...props}
      />
    </ContextMenuPrimitive.Portal>
  );
}

export function ContextMenuItem({
  className,
  ...props
}: React.ComponentProps<typeof ContextMenuPrimitive.Item>) {
  return (
    <ContextMenuPrimitive.Item
      className={[
        'flex cursor-pointer select-none items-center gap-2 rounded-md px-2 py-1.5 text-[13px] outline-none transition-colors',
        // Radix marks the focused item with `data-highlighted`, from either input.
        'focus:bg-item-hover data-[highlighted]:bg-item-hover data-[disabled]:pointer-events-none data-[disabled]:opacity-40',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      {...props}
    />
  );
}
