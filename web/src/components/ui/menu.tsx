"use client";

import * as M from "@radix-ui/react-dropdown-menu";
import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

/** Non-modal by default: a modal menu aria-hides the page while its controls stay focusable (axe aria-hidden-focus). */
export function Menu({ modal = false, ...props }: ComponentProps<typeof M.Root>) {
  return <M.Root modal={modal} {...props} />;
}
export const MenuTrigger = M.Trigger;

export function MenuContent({ className, ...props }: ComponentProps<typeof M.Content>) {
  return (
    <M.Portal>
      <M.Content
        align="start"
        sideOffset={4}
        className={cn(
          "z-50 min-w-56 rounded-control border border-line bg-surface p-1 text-ink shadow-pop outline-none",
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
  return <M.Label className="px-2 py-1 text-label font-semibold uppercase tracking-label text-muted">{children}</M.Label>;
}

/**
 * (P5-REFINE-3) A menu row with a switch on its right (`menuitemcheckbox`): toggles without closing the menu. The switch
 * is drawn like `Switch` (40×24) from the row's checked state.
 */
export function MenuSwitchItem({
  className,
  children,
  checked,
  onCheckedChange,
  disabled,
}: {
  className?: string;
  children: ReactNode;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <M.CheckboxItem
      checked={checked}
      disabled={disabled}
      onCheckedChange={onCheckedChange}
      onSelect={(e) => e.preventDefault()}
      className={cn(
        "flex min-h-9 cursor-pointer items-center gap-2.5 rounded-md px-2 py-1.5 text-sm outline-none data-[highlighted]:bg-subtle data-[disabled]:opacity-50",
        className,
      )}
    >
      <span className="min-w-0 flex-1">{children}</span>
      <span aria-hidden="true" className="relative inline-flex h-6 w-10 shrink-0">
        <span className={cn("absolute inset-0 rounded-full border transition-colors", checked ? "border-accent bg-accent" : "border-line-strong bg-subtle")} />
        <span className={cn("absolute left-1 top-1 size-4 rounded-full transition-transform motion-reduce:transition-none", checked ? "translate-x-4 bg-on-accent" : "bg-ink-2")} />
      </span>
    </M.CheckboxItem>
  );
}
