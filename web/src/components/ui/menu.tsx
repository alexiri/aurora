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
          "z-50 min-w-48 rounded-md border border-neutral-200 bg-white p-1 shadow-md",
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
        "cursor-pointer rounded px-2 py-1.5 text-sm outline-none data-[highlighted]:bg-neutral-100 data-[disabled]:opacity-50",
        className,
      )}
      {...props}
    />
  );
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return <M.Label className="px-2 py-1 text-xs font-medium text-neutral-500">{children}</M.Label>;
}
