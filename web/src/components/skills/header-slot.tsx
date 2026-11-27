"use client";

import { useMemo, useState } from "react";

import type { ScreenChrome } from "@/components/ui/list-detail";

/**
 * A page header that takes the list's **Add skill** as its action, and steps aside while a skill has its own screen on
 * a narrow screen (`ui/list-detail`'s chrome, as the agent's settings frame does it).
 */
export function useHeaderSlot() {
  const [actionSlot, setActionSlot] = useState<HTMLDivElement | null>(null);
  const [subScreen, setSubScreen] = useState(false);
  const chrome = useMemo<ScreenChrome>(() => ({ actionSlot, setSubScreen }), [actionSlot]);
  return { chrome, subScreen, slot: <div ref={setActionSlot} className="empty:hidden" /> };
}
