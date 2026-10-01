import * as React from 'react';
import * as DropdownMenuPrimitive from '@radix-ui/react-dropdown-menu';

export const DropdownMenu = DropdownMenuPrimitive.Root;
export const DropdownMenuTrigger = DropdownMenuPrimitive.Trigger;
export const DropdownMenuGroup = DropdownMenuPrimitive.Group;
export const DropdownMenuPortal = DropdownMenuPrimitive.Portal;
export const DropdownMenuSub = DropdownMenuPrimitive.Sub;

export function DropdownMenuContent({
  className,
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        sideOffset={sideOffset}
        className={[
          'z-50 min-w-[180px] overflow-hidden rounded-lg border border-border bg-background p-1 text-foreground shadow-[0_4px_16px_rgba(0,0,0,0.14)] outline-none dark:shadow-[0_4px_16px_rgba(0,0,0,0.5)]',
          className,
        ]
          .filter(Boolean)
          .join(' ')}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

export function DropdownMenuItem({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Item>) {
  return (
    <DropdownMenuPrimitive.Item
      className={[
        'flex cursor-pointer select-none items-center gap-2 rounded-md px-2 py-1.5 text-[13px] outline-none transition-colors',
        // `data-highlighted` is what Radix sets on keyboard and pointer focus.
        'focus:bg-item-hover data-[highlighted]:bg-item-hover data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      {...props}
    />
  );
}

// Destructive actions, kept apart from the neutral ones so they read as a
// different kind of choice rather than just another row.
export function DropdownMenuItemDestructive({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Item>) {
  return (
    <DropdownMenuPrimitive.Item
      className={[
        'flex cursor-pointer select-none items-center gap-2 rounded-md px-2 py-1.5 text-[13px] text-destructive outline-none transition-colors',
        'focus:bg-destructive/10 data-[highlighted]:bg-destructive/10 data-[disabled]:pointer-events-none data-[disabled]:opacity-50',
        className,
      ]
        .filter(Boolean)
        .join(' ')}
      {...props}
    />
  );
}

export function DropdownMenuSeparator({
  className,
  ...props
}: React.ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return (
    <DropdownMenuPrimitive.Separator
      className={['-mx-1 my-1 h-px bg-border', className].filter(Boolean).join(' ')}
      {...props}
    />
  );
}

export function DropdownMenuShortcut({
  className,
  ...props
}: React.ComponentProps<'span'>) {
  return (
    <span
      className={['ml-auto text-xs tracking-widest text-muted-foreground', className]
        .filter(Boolean)
        .join(' ')}
      {...props}
    />
  );
}
