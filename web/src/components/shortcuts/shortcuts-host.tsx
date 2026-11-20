"use client";

import { useParams, useRouter } from "next/navigation";
import { useEffect, useState } from "react";

import { openPalette, SHEET_OPEN_EVENT, usePaletteContext } from "@/components/palette/command-palette";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { requestVerb } from "@/lib/palette-verb";
import { keysLabel, SHORTCUT_GROUPS, SHORTCUTS, useShortcuts, type Shortcut } from "@/lib/shortcuts";
import { wsRoutes } from "@/lib/routes";

/** A modal dialog is open that is not the palette or the sheet (they carry `data-shortcut-ok`). */
function otherDialogOpen(): boolean {
  return document.querySelector("[role='dialog']:not([data-shortcut-ok]), [role='alertdialog']") !== null;
}

/** The approval card the focus is in (the chat card or a card of the Approvals page), and one of its two answers. */
export function approvalButton(active: Element | null, key: "allow-once" | "block"): HTMLButtonElement | null {
  const card = active?.closest("[data-approval-card]");
  const button = card?.querySelector<HTMLButtonElement>(`[data-approval-key='${key}']`);
  return button && !button.disabled ? button : null;
}

/**
 * (P5-REFINE-19) Mounted once in the app shell: the one keydown listener, and the shortcut sheet. With "Show more controls"
 * off only the palette key works (`lib/shortcuts.ts` has the rule that decides when a key may fire).
 */
export function ShortcutsHost() {
  const router = useRouter();
  const params = useParams<{ org?: string; ws?: string; handle?: string }>();
  const ctx = usePaletteContext();
  const [sheet, setSheet] = useState(false);

  useEffect(() => {
    const on = () => setSheet(true);
    window.addEventListener(SHEET_OPEN_EVENT, on);
    return () => window.removeEventListener(SHEET_OPEN_EVENT, on);
  }, []);

  const org = params?.org;
  const ws = params?.ws;
  function onShortcut(s: Shortcut) {
    const routes = org && ws ? wsRoutes(org, ws) : null;
    switch (s.id) {
      case "sheet":
        setSheet(true);
        break;
      case "go-agents":
        if (routes && ctx.topics.includes("agents")) router.push(routes.agents());
        break;
      case "go-approvals":
        if (routes && ctx.topics.includes("approvals")) router.push(routes.approvals());
        break;
      case "go-overview":
        if (routes) router.push(routes.home());
        break;
      case "go-settings":
        if (routes && ctx.topics.includes("settings")) router.push(routes.settings());
        break;
      case "new-chat": {
        const agent = ctx.agents.find((a) => a.handle === params?.handle);
        if (routes && agent && !agent.byo) {
          router.push(routes.agent(agent.handle));
          requestVerb("new-chat");
        }
        break;
      }
      case "allow-once":
      case "block":
        // Only a card that has the focus: the same button, the same confirmation (a parked approval still asks).
        approvalButton(document.activeElement, s.id)?.click();
        break;
      default:
        break;
    }
  }

  useShortcuts({ enabled: ctx.verbs, onPalette: openPalette, onShortcut, dialogOpen: otherDialogOpen });

  return <ShortcutSheet open={sheet} onOpenChange={setSheet} />;
}

export function ShortcutSheet({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const mac = typeof navigator !== "undefined" && /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title="Keyboard shortcuts"
        description="Letter shortcuts never fire while you type, in the terminal or in an editor."
        size="md"
        data-shortcut-ok=""
        data-testid="shortcut-sheet"
      >
        <div className="space-y-4">
          {SHORTCUT_GROUPS.map((g) => (
            <section key={g} aria-labelledby={`sc-${g}`}>
              <h3 id={`sc-${g}`} className="pb-1 text-label font-semibold uppercase tracking-label text-muted">
                {g}
              </h3>
              <dl className="divide-y divide-line">
                {SHORTCUTS.filter((s) => s.group === g).map((s) => (
                  <div key={s.id} className="flex items-center justify-between gap-3 py-2 text-sm">
                    <dt className="min-w-0 text-ink-2">{s.label}</dt>
                    <dd className="shrink-0">
                      <kbd className="rounded-control border border-line-strong bg-subtle px-1.5 py-0.5 font-mono text-xs text-ink">{keysLabel(s.keys, mac)}</kbd>
                    </dd>
                  </div>
                ))}
              </dl>
            </section>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
