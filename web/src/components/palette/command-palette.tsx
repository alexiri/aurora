"use client";

import * as DialogPrimitive from "@radix-ui/react-dialog";
import { useParams, useRouter } from "next/navigation";
import { useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";

import { availableSections, type AgentPageId } from "@/components/agents/sections";
import { SearchIcon } from "@/components/ui/icons";
import { useAltitude } from "@/lib/altitude";
import { visibleTopics } from "@/lib/api/roles";
import { useAgents, useInstance, useMe, useOrg, useResolve, useWorkspace } from "@/lib/api/queries";
import { readRecent } from "@/lib/nav/recent-scopes";
import { matchItems, paletteItems, type PaletteContext, type PaletteGroup, type PaletteItem } from "@/lib/palette-registry";
import { requestVerb } from "@/lib/palette-verb";
import { keysLabel, SHORTCUTS } from "@/lib/shortcuts";
import { cn } from "@/lib/utils";

export const PALETTE_OPEN_EVENT = "aurora:palette-open";
export const SHEET_OPEN_EVENT = "aurora:shortcut-sheet-open";

/** Open the palette from anywhere (the rail's button, the `⌘K` handler). */
export function openPalette(): void {
  window.dispatchEvent(new Event(PALETTE_OPEN_EVENT));
}
export function openShortcutSheet(): void {
  window.dispatchEvent(new Event(SHEET_OPEN_EVENT));
}

/** The viewer's data in the shape the registry takes. Everything here is already cached by the shell. */
export function usePaletteContext(): PaletteContext {
  const params = useParams<{ org?: string; ws?: string; handle?: string }>();
  const orgSlug = params?.org;
  const wsSlug = params?.ws;
  const me = useMe();
  const resolved = useResolve(orgSlug, wsSlug);
  const wsId = resolved.data?.workspace?.id;
  const workspace = useWorkspace(wsId);
  const agents = useAgents(wsId);
  const instance = useInstance();
  const org = useOrg(resolved.data?.org.id);
  const altitude = useAltitude({ wsId });
  const instanceAdmin = me.data?.user.is_instance_admin === true;
  const role = workspace.data?.my_role;
  const handle = params?.handle;

  return useMemo(() => {
    const agentList = (agents.data?.items ?? []).map((a) => ({ id: a.id, name: a.name, handle: a.handle, paused: a.frozen != null, byo: a.kind === "byo" }));
    const open = agentList.find((a) => a.handle === handle);
    const av = {
      byo: open?.byo === true,
      previews: instance.data?.features.previews === true && org.data?.settings.previews_enabled === true,
      terminal: instance.data?.features.terminal === true,
      ssh: instance.data?.features.ssh === true,
    };
    const pages = open ? availableSections(av, altitude.allows).flatMap((s) => s.pages) : [];
    return {
      verbs: altitude.allows("expert"),
      orgSlug,
      wsSlug,
      agentHandle: open?.handle,
      memberships: (me.data?.memberships ?? []).map((m) => ({
        slug: m.org.slug,
        name: m.org.name ?? m.org.slug,
        role: m.role,
        workspaces: m.workspaces.map((w) => ({ slug: w.slug, name: w.name ?? w.slug })),
      })),
      topics: wsId ? visibleTopics("workspace", { instanceAdmin, workspace: role ?? (instanceAdmin ? undefined : "member") }, altitude) : [],
      agents: wsId ? agentList : [],
      agentPages: pages.map((p: { id: AgentPageId; label: string }) => ({ id: p.id, label: p.label })),
      role,
      instanceAdmin,
      ssh: instance.data?.features.ssh === true,
    } satisfies PaletteContext;
  }, [orgSlug, wsSlug, handle, me.data, agents.data, instance.data, org.data, altitude, wsId, role, instanceAdmin]);
}

/** ⌘K / Ctrl K: a search field and one list of places (and, with more controls, actions). Choosing one goes there. */
export function CommandPalette() {
  const router = useRouter();
  const ctx = usePaletteContext();
  const me = useMe();
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listId = useId();
  const input = useRef<HTMLInputElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const loading = me.isPending;

  useEffect(() => {
    const on = () => setOpen(true);
    window.addEventListener(PALETTE_OPEN_EVENT, on);
    return () => window.removeEventListener(PALETTE_OPEN_EVENT, on);
  }, []);

  // Read once per opening: the order must not shuffle while the person types.
  const viewerId = me.data?.user.id;
  const recent = useMemo(() => (open ? readRecent(viewerId) : []), [open, viewerId]);
  const all = useMemo(() => paletteItems(ctx), [ctx]);
  const results = useMemo(() => matchItems(all, query, recent), [all, query, recent]);
  const clamped = Math.min(active, Math.max(0, results.length - 1));

  useEffect(() => {
    list.current?.querySelector<HTMLElement>("[aria-selected='true']")?.scrollIntoView?.({ block: "nearest" });
  }, [clamped, results]);

  function change(next: boolean) {
    setOpen(next);
    if (!next) {
      setQuery("");
      setActive(0);
    }
  }

  function choose(item: PaletteItem) {
    change(false);
    if (item.verb === "shortcuts") {
      openShortcutSheet();
      return;
    }
    if (item.href) router.push(item.href);
    // The verb waits for the page that owns the button and opens the button's own dialog.
    if (item.verb) requestVerb(item.verb);
  }

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActive(Math.min(results.length - 1, clamped + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActive(Math.max(0, clamped - 1));
    } else if (e.key === "Home") {
      e.preventDefault();
      setActive(0);
    } else if (e.key === "End") {
      e.preventDefault();
      setActive(Math.max(0, results.length - 1));
    } else if (e.key === "Enter" && !e.nativeEvent.isComposing) {
      e.preventDefault();
      const item = results[clamped];
      if (item) choose(item);
    }
  }

  const groups: { group: PaletteGroup; items: { item: PaletteItem; index: number }[] }[] = [];
  results.forEach((item, index) => {
    const last = groups[groups.length - 1];
    if (last && last.group === item.group) last.items.push({ item, index });
    else groups.push({ group: item.group, items: [{ item, index }] });
  });

  const mac = typeof navigator !== "undefined" && /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent);
  const paletteKeys = keysLabel(SHORTCUTS.find((s) => s.id === "palette")!.keys, mac);

  return (
    <DialogPrimitive.Root open={open} onOpenChange={change}>
      <DialogPrimitive.Portal>
        <DialogPrimitive.Overlay className="fixed inset-0 z-40 bg-scrim" />
        <DialogPrimitive.Content
          aria-describedby={undefined}
          data-testid="command-palette"
          data-shortcut-ok=""
          // Full width under a 1 rem gutter on a phone, top-anchored (as the dialogs are); 36 rem wide from `sm`.
          className="fixed inset-x-4 top-4 z-50 flex max-h-[80dvh] flex-col overflow-hidden rounded-card border border-line bg-surface text-ink shadow-pop outline-none sm:inset-x-auto sm:left-1/2 sm:top-[12vh] sm:w-full sm:max-w-xl sm:-translate-x-1/2"
        >
          <DialogPrimitive.Title className="sr-only">Search Aurora</DialogPrimitive.Title>
          <div className="flex shrink-0 items-center gap-2 border-b border-line px-3">
            <SearchIcon className="size-4 shrink-0 text-muted" aria-hidden="true" />
            <input
              ref={input}
              role="combobox"
              aria-expanded="true"
              aria-controls={listId}
              aria-activedescendant={results[clamped] ? `${listId}-${clamped}` : undefined}
              aria-label="Search places and actions"
              autoComplete="off"
              spellCheck={false}
              placeholder={ctx.verbs ? "Go to a place or do something…" : "Go to a workspace, an agent or a page…"}
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setActive(0);
              }}
              onKeyDown={onKeyDown}
              className="min-h-12 min-w-0 flex-1 bg-transparent text-sm text-ink outline-none placeholder:text-muted"
            />
          </div>
          <div ref={list} id={listId} role="listbox" aria-label="Results" className="min-h-0 flex-1 overflow-y-auto p-1.5">
            {loading ? (
              <div role="status" aria-live="polite" aria-busy="true" data-testid="palette-skeleton">
                <span className="sr-only">Loading…</span>
                <span aria-hidden="true" className="m-2 block h-8 animate-pulse rounded-control bg-subtle motion-reduce:animate-none" />
              </div>
            ) : results.length === 0 ? (
              <p role="status" className="px-3 py-6 text-center text-sm text-ink-2">
                Nothing matches “{query.trim()}”.
              </p>
            ) : (
              groups.map((g) => (
                <div key={g.group} role="group" aria-labelledby={`${listId}-g-${g.group}`}>
                  <p id={`${listId}-g-${g.group}`} className="px-2 pb-1 pt-2 text-label font-semibold uppercase tracking-label text-muted">
                    {g.group}
                  </p>
                  {g.items.map(({ item, index }) => (
                    <div
                      key={item.id}
                      id={`${listId}-${index}`}
                      role="option"
                      aria-selected={index === clamped}
                      data-palette-item={item.id}
                      onMouseMove={() => setActive(index)}
                      onClick={() => choose(item)}
                      className={cn(
                        "flex min-h-11 cursor-pointer items-center gap-2 rounded-md px-2 py-1.5 text-sm sm:min-h-9",
                        index === clamped ? "bg-subtle text-ink" : "text-ink-2",
                      )}
                    >
                      <span className="min-w-0 flex-1 truncate">{item.label}</span>
                      {item.hint ? <span className="hidden max-w-48 shrink-0 truncate text-xs text-muted sm:inline">{item.hint}</span> : null}
                    </div>
                  ))}
                </div>
              ))
            )}
          </div>
          <p className="shrink-0 border-t border-line px-3 py-2 text-xs text-muted">
            <span aria-hidden="true">↑↓ to move · Enter to choose · Esc to close</span>
            <span className="sr-only">Use the arrow keys to move, Enter to choose, Escape to close. Open this from anywhere with {paletteKeys}.</span>
          </p>
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  );
}
