import * as React from 'react';
import * as TooltipPrimitive from '@radix-ui/react-tooltip';
import { cn } from '@/lib/utils';

export const TooltipProvider = TooltipPrimitive.Provider;
export const Tooltip = TooltipPrimitive.Root;
export const TooltipTrigger = TooltipPrimitive.Trigger;

export function TooltipContent({
  className,
  sideOffset = 4,
  ...props
}: React.ComponentProps<typeof TooltipPrimitive.Content>) {
  return (
    <TooltipPrimitive.Portal>
      <TooltipPrimitive.Content
        sideOffset={sideOffset}
        className={cn(
          'z-50 max-w-[280px] rounded-lg border border-border bg-background px-3 py-2 text-foreground shadow-[0_4px_16px_rgba(0,0,0,0.14)] outline-none dark:shadow-[0_4px_16px_rgba(0,0,0,0.5)]',
          className,
        )}
        {...props}
      />
    </TooltipPrimitive.Portal>
  );
}
