"use client";

import { useEffect, useRef, type ButtonHTMLAttributes, type KeyboardEvent, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/** A key cap inside a tooltip or a hint (`Ctrl` `B`). */
export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <kbd
      className={cn(
        "inline-flex min-w-5 items-center justify-center rounded border border-line-strong bg-subtle px-1 font-mono text-xs leading-4 text-ink-2",
        className,
      )}>
      {children}
    </kbd>
  );
}

/**
 * A hover/focus label above its trigger. The trigger keeps its own accessible name (which already includes the
 * shortcut), so the bubble is hidden from assistive technology.
 */
export function Tooltip({ label, keys, bubbleClassName, children }: { label: string; keys?: string[]; bubbleClassName?: string; children: ReactNode }) {
  return (
    <span className="group/tip relative inline-flex">
      {children}
      <span
        aria-hidden="true"
        className={cn(
          "pointer-events-none absolute bottom-full left-1/2 z-30 mb-1 hidden -translate-x-1/2 items-center gap-1 whitespace-nowrap rounded-control border border-line bg-surface px-2 py-1 text-xs text-ink shadow-pop md:group-focus-within/tip:flex md:group-hover/tip:flex",
          bubbleClassName,
        )}>
        {label}
        {keys?.map((k) => (
          <Kbd key={k}>{k}</Kbd>
        ))}
      </span>
    </span>
  );
}

/**
 * `role="toolbar"` with roving focus: one tab stop, Left/Right (Up/Down) move between the buttons, Home/End jump.
 * Items are any element marked `data-toolbar-item` (use `ToolbarButton`); disabled ones are skipped.
 */
export function Toolbar({
  label,
  className,
  children,
  orientation = "horizontal",
}: {
  label: string;
  className?: string;
  children: ReactNode;
  orientation?: "horizontal" | "vertical";
}) {
  const ref = useRef<HTMLDivElement>(null);
  const items = () => Array.from(ref.current?.querySelectorAll<HTMLElement>("[data-toolbar-item]:not(:disabled)") ?? []);
  const rove = (to: HTMLElement | undefined) => {
    for (const el of ref.current?.querySelectorAll<HTMLElement>("[data-toolbar-item]") ?? []) el.tabIndex = el === to ? 0 : -1;
  };
  // Keep exactly one enabled item in the tab order after every render (buttons come and go with Write / Preview).
  useEffect(() => {
    const all = items();
    rove(all.find((el) => el.tabIndex === 0) ?? all[0]);
  });
  function onKeyDown(e: KeyboardEvent<HTMLDivElement>) {
    const next = orientation === "horizontal" ? "ArrowRight" : "ArrowDown";
    const prev = orientation === "horizontal" ? "ArrowLeft" : "ArrowUp";
    if (![next, prev, "Home", "End"].includes(e.key)) return;
    const all = items();
    const at = all.indexOf(document.activeElement as HTMLElement);
    if (at < 0) return;
    e.preventDefault();
    const to = e.key === "Home" ? all[0] : e.key === "End" ? all[all.length - 1] : all[(at + (e.key === next ? 1 : -1) + all.length) % all.length];
    rove(to);
    to?.focus();
  }
  return (
    <div
      ref={ref}
      role="toolbar"
      aria-label={label}
      aria-orientation={orientation}
      onKeyDown={onKeyDown}
      onFocus={(e) => {
        if (e.target instanceof HTMLElement && e.target.hasAttribute("data-toolbar-item")) rove(e.target);
      }}
      className={cn("flex items-center gap-0.5", className)}>
      {children}
    </div>
  );
}

export function ToolbarSeparator() {
  return <span role="separator" aria-orientation="vertical" className="mx-1 h-5 w-px shrink-0 bg-line" />;
}

/** A 44 px (36 px from 768 px) icon button for a `Toolbar`; `pressed` makes it a toggle. */
export function ToolbarButton({
  label,
  keys,
  ariaKeys,
  pressed,
  className,
  children,
  ...rest
}: Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label" | "aria-pressed"> & {
  /** The action's name. With `keys` the accessible name becomes `Label (Ctrl+B)`. */
  label: string;
  keys?: string[];
  ariaKeys?: string;
  pressed?: boolean;
}) {
  return (
    <Tooltip label={label} keys={keys}>
      <button
        type="button"
        data-toolbar-item=""
        aria-label={keys ? `${label} (${keys.join("+")})` : label}
        aria-keyshortcuts={ariaKeys}
        aria-pressed={pressed}
        className={cn(
          "inline-flex size-11 shrink-0 items-center justify-center rounded-control text-ink-2 hover:bg-subtle hover:text-ink focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent disabled:pointer-events-none disabled:opacity-50 aria-pressed:bg-subtle aria-pressed:text-ink md:size-9",
          className,
        )}
        {...rest}>
        {children}
      </button>
    </Tooltip>
  );
}
