"use client";

import * as M from "@radix-ui/react-dropdown-menu";
import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

export const Menu = M.Root;
export const MenuTrigger = M.Trigger;

export function MenuContent({ className, ...props }: ComponentProps<typeof M.Content>) {
  return (
    <M.Portal>
      <M.Content
        align="start"
        sideOffset={4}
        className={cn(
          "z-50 min-w-56 rounded-control border border-line bg-surface p-1 text-ink shadow-pop",
          className,
        )}
        {...props}
      />
    </M.Portal>
  );
}

export function MenuItem({ className, ...props }: ComponentProps<typeof M.Item>) {
  return (
    <M.Item
      className={cn(
        "flex min-h-9 cursor-pointer items-center rounded-md px-2 py-1.5 text-sm outline-none data-[highlighted]:bg-subtle data-[disabled]:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

export function MenuSeparator() {
  return <M.Separator className="my-1 h-px bg-line" />;
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return <M.Label className="px-2 py-1 text-[11px] font-semibold uppercase tracking-[0.06em] text-muted">{children}</M.Label>;
}
