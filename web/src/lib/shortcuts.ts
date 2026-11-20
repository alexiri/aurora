"use client";

import { useEffect, useRef } from "react";

/**
 * (P5-REFINE-19, D-406) The keyboard shortcuts: one registry, one matcher, one `keydown` listener on `document`.
 * Everything here except `⌘K`/`Ctrl K` and `/` belongs to Expert (behind "Show more controls"). The rule that keeps them
 * from ever firing by accident lives in `shortcutBlocked`: typing targets, the terminal and editors, a modifier, an open
 * dialog and a touch-only device never fire a letter shortcut.
 */

export type ShortcutId =
  | "palette"
  | "search"
  | "go-agents"
  | "go-approvals"
  | "go-overview"
  | "go-settings"
  | "new-chat"
  | "allow-once"
  | "block"
  | "sheet";

export interface Shortcut {
  id: ShortcutId;
  /** One key, or a chord (`["g", "a"]`: `g` then `a` within a second). */
  keys: string[];
  label: string;
  group: "Anywhere" | "Go to" | "Agent" | "Approvals";
  /** Behind "Show more controls". `⌘K` and `/` are not. */
  expert: boolean;
}

export const CHORD_WINDOW_MS = 1000;

export const SHORTCUTS: readonly Shortcut[] = [
  { id: "palette", keys: ["mod", "k"], label: "Open the command palette", group: "Anywhere", expert: false },
  { id: "search", keys: ["/"], label: "Search this conversation", group: "Anywhere", expert: false },
  { id: "sheet", keys: ["?"], label: "Show the keyboard shortcuts", group: "Anywhere", expert: true },
  { id: "go-agents", keys: ["g", "a"], label: "Go to Agents", group: "Go to", expert: true },
  { id: "go-approvals", keys: ["g", "p"], label: "Go to Approvals", group: "Go to", expert: true },
  { id: "go-overview", keys: ["g", "o"], label: "Go to Overview", group: "Go to", expert: true },
  { id: "go-settings", keys: ["g", "s"], label: "Go to Settings", group: "Go to", expert: true },
  { id: "new-chat", keys: ["n"], label: "New chat with this agent", group: "Agent", expert: true },
  { id: "allow-once", keys: ["a"], label: "Allow once (on the selected approval)", group: "Approvals", expert: true },
  { id: "block", keys: ["b"], label: "Block (on the selected approval)", group: "Approvals", expert: true },
];

export const SHORTCUT_GROUPS: Shortcut["group"][] = ["Anywhere", "Go to", "Agent", "Approvals"];

/** `⌘` on a Mac, `Ctrl` elsewhere. */
export function isMac(platform: string = typeof navigator === "undefined" ? "" : navigator.platform || navigator.userAgent): boolean {
  return /mac|iphone|ipad/i.test(platform);
}

/** How a shortcut's keys read on the sheet and in the rail: `Ctrl K`, `⌘K`, `g then a`. */
export function keysLabel(keys: string[], mac = isMac()): string {
  if (keys[0] === "mod") return mac ? `⌘${keys[1]!.toUpperCase()}` : `Ctrl ${keys[1]!.toUpperCase()}`;
  return keys.length > 1 ? keys.join(" then ") : keys[0]!;
}

/** `⌘K` / `Ctrl K` (no other modifier). */
export function isPaletteKey(e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">): boolean {
  return (e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k";
}

const TYPING = "input, textarea, select, [contenteditable]:not([contenteditable='false']), [data-no-shortcuts]";

/** Focus is somewhere a letter is text: a field, a select, an editable, the terminal, an editor. */
export function isTypingTarget(target: EventTarget | null): boolean {
  const el = target instanceof Element ? target : null;
  return el !== null && el.closest(TYPING) !== null;
}

/** The terminal and the editors keep `⌘K` too (it is a terminal key); a plain text field does not. */
export function paletteBlocked(target: EventTarget | null): boolean {
  const el = target instanceof Element ? target : null;
  return el !== null && el.closest("[data-no-shortcuts]") !== null;
}

/** A device whose primary input cannot hover is a touch screen: a letter there is the on-screen keyboard's. */
export function touchOnly(): boolean {
  if (typeof window === "undefined" || typeof window.matchMedia !== "function") return false;
  return !window.matchMedia("(hover: hover)").matches;
}

export interface BlockContext {
  target: EventTarget | null;
  /** A modal dialog other than the sheet and the palette is open. */
  dialogOpen: boolean;
  touch: boolean;
}

/** True when a letter shortcut must not fire for this event. */
export function shortcutBlocked(e: Pick<KeyboardEvent, "metaKey" | "ctrlKey" | "altKey" | "repeat" | "isComposing">, ctx: BlockContext): boolean {
  if (e.metaKey || e.ctrlKey || e.altKey) return true;
  if (e.repeat || e.isComposing) return true;
  if (ctx.touch || ctx.dialogOpen) return true;
  return isTypingTarget(ctx.target);
}

/**
 * A chord matcher: feed it one key at a time. A single key shortcut fires at once unless it is the first key of a chord
 * (`g`), which waits one second for the second key; a second key that matches no chord is not a shortcut (it may be one
 * on its own: `g` then `n` is `n`'s own).
 */
export class ChordMatcher {
  private first: string | null = null;
  private at = 0;
  constructor(
    private readonly shortcuts: readonly Shortcut[],
    private readonly windowMs = CHORD_WINDOW_MS,
  ) {}

  feed(key: string, now: number): Shortcut | null {
    const k = key.length === 1 ? key : key;
    if (this.first !== null && now - this.at <= this.windowMs) {
      const hit = this.shortcuts.find((s) => s.keys.length === 2 && s.keys[0] === this.first && s.keys[1] === k);
      this.first = null;
      if (hit) return hit;
    } else {
      this.first = null;
    }
    if (this.shortcuts.some((s) => s.keys.length === 2 && s.keys[0] === k)) {
      this.first = k;
      this.at = now;
      return null;
    }
    return this.shortcuts.find((s) => s.keys.length === 1 && s.keys[0] === k) ?? null;
  }

  reset(): void {
    this.first = null;
  }
}

/**
 * The one listener. `enabled` is the Expert switch: with it off only the palette key reaches `onPalette`. `onShortcut`
 * returns nothing; the host decides what each id does (it knows the route). Events already handled
 * (`defaultPrevented`) are left alone.
 */
export function useShortcuts({
  enabled,
  onPalette,
  onShortcut,
  dialogOpen,
}: {
  enabled: boolean;
  onPalette: () => void;
  onShortcut: (s: Shortcut) => void;
  dialogOpen: () => boolean;
}): void {
  const latest = useRef({ enabled, onPalette, onShortcut, dialogOpen });
  useEffect(() => {
    latest.current = { enabled, onPalette, onShortcut, dialogOpen };
  });
  useEffect(() => {
    const matcher = new ChordMatcher(SHORTCUTS.filter((s) => s.expert && s.id !== "search" && s.id !== "palette"));
    const onKey = (e: KeyboardEvent) => {
      const cur = latest.current;
      if (e.defaultPrevented) return;
      if (isPaletteKey(e)) {
        if (paletteBlocked(e.target)) return;
        e.preventDefault();
        cur.onPalette();
        return;
      }
      if (!cur.enabled) return;
      if (e.key === "Shift" || e.key === "Meta" || e.key === "Control" || e.key === "Alt") return;
      const blocked = shortcutBlocked(e, { target: e.target, dialogOpen: cur.dialogOpen(), touch: touchOnly() });
      if (blocked) {
        matcher.reset();
        return;
      }
      const hit = matcher.feed(e.key, Date.now());
      if (hit) cur.onShortcut(hit);
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);
}
