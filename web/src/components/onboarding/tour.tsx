"use client";

import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import { usePatchOnboarding } from "@/lib/api/onboarding";
import { cn } from "@/lib/utils";

import { TOUR_EVENT } from "./tour-events";

export interface TourStop {
  /** `data-tour` of the navigation link it points at. */
  target: string;
  title: string;
  text: string;
}

/** The coach marks (`14 §Onboarding` 3): one or two sentences each, on what lives there and when you need it. */
/**
 * One stop per workspace topic (C-975, P5-IA-1 scope 9), in navigation order. A stop is shown only when its link is on
 * screen, so the tour follows `visibleTopics`: a member gets no Approvals, People or Settings. `**x**` is bold.
 */
export const TOUR_STOPS: readonly TourStop[] = [
  { target: "overview", title: "Overview", text: "What your agents are doing, and what needs you." },
  { target: "agents", title: "Agents", text: "Chat with an agent here. Open an agent, then **Apps & access** › **Give <Agent> an app** to let it use GitHub, Linear and more." },
  { target: "apps", title: "Apps & accounts", text: "The accounts your agents can use, and the apps you can connect." },
  { target: "internet-access", title: "Internet access", text: "Which websites agents can reach, and the rules that ask or block." },
  { target: "approvals", title: "Approvals", text: "When an agent asks before a change, you answer here." },
  { target: "activity", title: "Activity", text: "Everything agents did, with what was allowed, asked and blocked." },
  { target: "people", title: "People", text: "Who can use this workspace." },
  { target: "settings", title: "Settings", text: "The workspace's name, API keys and audit log." },
];

/** A stop's sentence with its `**bold**` parts as <strong>. */
function stopText(text: string) {
  return text.split(/\*\*(.+?)\*\*/g).map((part, i) => (i % 2 === 1 ? <strong key={i}>{part}</strong> : part));
}

const NARROW = "(max-width: 767px)";

/** The navigation link a stop points at, if it is on screen (the sidebar and the drawer both carry one). */
function findTarget(target: string): HTMLElement | null {
  const all = document.querySelectorAll<HTMLElement>(`[data-tour="${target}"]`);
  for (const el of all) if (el.getClientRects().length > 0) return el;
  return null;
}

/** Anything that takes text or arrows itself: the tour never handles keys typed there. */
function isEditable(el: Element): boolean {
  return el.closest("input, textarea, select, [contenteditable]:not([contenteditable='false'])") !== null;
}

/** Open dialogs and menus other than the tour's own card (which is `data-testid="tour-card"`). */
const OTHER_OVERLAY =
  "dialog[open], [role='dialog']:not([data-testid='tour-card']), [role='alertdialog'], [role='menu'], [role='listbox']";

/**
 * Coach marks over the real navigation. Keyboard: Next (→ or the Next button), Back (←), Esc to leave. Focus moves to the card
 * at every stop and returns where it was at the end. No animation, so reduced motion needs nothing. At 390 px the
 * drawer opens as the tour goes and the card sits at the bottom of the screen.
 */
export function Tour({ setNavOpen }: { setNavOpen: (open: boolean) => void }) {
  const patch = usePatchOnboarding();
  const [stops, setStops] = useState<TourStop[] | null>(null);
  const [index, setIndex] = useState(0);
  const [narrow, setNarrow] = useState(false);
  const card = useRef<HTMLDivElement>(null);
  const spot = useRef<HTMLDivElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);
  const markDone = patch.mutate;

  const close = useCallback(
    (done: boolean) => {
      setStops(null);
      setIndex(0);
      setNavOpen(false);
      if (done) markDone({ tour_completed: true });
      returnFocus.current?.focus();
      returnFocus.current = null;
    },
    [markDone, setNavOpen],
  );

  // Start on the event (the Welcome page, the account menu) and on `?tour=1` (the menu from a page without the navigation).
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cancelled = false;
    function start() {
      returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      const isNarrow = window.matchMedia(NARROW).matches;
      setNarrow(isNarrow);
      if (isNarrow) setNavOpen(true);
      // The links may still be loading (names, roles): look for a moment, then go with what is there.
      let tries = 0;
      const look = () => {
        if (cancelled) return;
        const found = TOUR_STOPS.filter((s) => findTarget(s.target));
        if (found.length >= 6 || tries++ > 20) {
          if (found.length === 0) return close(false);
          setIndex(0);
          setStops(found);
          return;
        }
        timer = setTimeout(look, 150);
      };
      timer = setTimeout(look, 50);
    }
    window.addEventListener(TOUR_EVENT, start);
    const url = new URL(window.location.href);
    if (url.searchParams.get("tour") === "1") {
      url.searchParams.delete("tour");
      window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
      start();
    }
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      window.removeEventListener(TOUR_EVENT, start);
    };
  }, [close, setNavOpen]);

  const stop = stops?.[index];

  // Point at the stop: mark the link, place the card next to it (wide) or leave it at the bottom (narrow).
  useLayoutEffect(() => {
    if (!stop) return;
    const el = findTarget(stop.target);
    if (!el) return;
    el.setAttribute("data-tour-active", "true");
    el.scrollIntoView({ block: "nearest" });
    const c = card.current;
    if (c && !narrow) {
      const r = el.getBoundingClientRect();
      const top = Math.max(8, Math.min(r.top - 8, window.innerHeight - c.offsetHeight - 8));
      c.style.setProperty("left", `${Math.round(r.right + 12)}px`);
      c.style.setProperty("top", `${Math.round(top)}px`);
    }
    c?.focus();
    // (firstday-4) The page is dimmed around the link: a cut-out the size of the link, kept in step while the page moves. Set
    // through the CSSOM (a style attribute is not allowed by the CSP guard).
    const place = () => {
      const sp = spot.current;
      if (!sp) return;
      const r = el.getBoundingClientRect();
      sp.style.setProperty("left", `${Math.round(r.left - 3)}px`);
      sp.style.setProperty("top", `${Math.round(r.top - 3)}px`);
      sp.style.setProperty("width", `${Math.round(r.width + 6)}px`);
      sp.style.setProperty("height", `${Math.round(r.height + 6)}px`);
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      el.removeAttribute("data-tour-active");
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [stop, narrow]);

  // A layout effect, not a passive one: the listener is attached in the same commit that shows the card,
  // so the dialog is never on screen without its keys (a passive effect could run after a key was
  // already pressed — the FIX-067 test caught that on CI).
  useLayoutEffect(() => {
    if (!stops) return;
    const last = stops.length - 1;
    function onKey(e: KeyboardEvent) {
      const target = e.target instanceof Element ? e.target : null;
      // Another open dialog or menu owns the keyboard: leave its keys alone.
      if (target?.closest(OTHER_OVERLAY)) return;
      // Arrows move the caret in a field; never take them from there.
      if (e.key !== "Escape" && target && isEditable(target)) return;
      if (e.key === "Escape") {
        // Leaving with Esc is not finishing: only Done and Skip tour record the tour (docs/14 §Onboarding
        // 3 lists Esc as the way out and the tour as replayable; a stray Esc must not retire it).
        e.preventDefault();
        close(false);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        if (index < last) setIndex(index + 1);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        if (index > 0) setIndex(index - 1);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [stops, index, close]);

  if (!stops || !stop) return null;
  const last = index === stops.length - 1;
  return (
    <>
    {/* The dimmed page: no overlay catches clicks or keys (nothing is inert), so Esc and the arrows still work. The link stays bright through the cut-out. */}
    <div
      ref={spot}
      aria-hidden="true"
      data-testid="tour-backdrop"
      className="pointer-events-none fixed z-[58] rounded-control ring-2 ring-accent shadow-[0_0_0_100vmax_var(--scrim)]"
    />
    <div
      ref={card}
      role="dialog"
      aria-modal="false"
      aria-labelledby="tour-title"
      aria-describedby="tour-text"
      tabIndex={-1}
      data-testid="tour-card"
      className={cn(
        "fixed z-[60] rounded-card border border-line-strong bg-surface p-4 text-ink shadow-pop outline-none",
        narrow ? "inset-x-4 bottom-4" : "w-80",
      )}
    >
      <p className="text-xs font-semibold uppercase tracking-label text-muted">
        Step {index + 1} of {stops.length}
      </p>
      <h2 id="tour-title" className="mt-1 font-display text-base font-semibold">
        {stop.title}
      </h2>
      <p id="tour-text" className="mt-1 text-sm text-ink-2">
        {stopText(stop.text)}
      </p>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Button size="sm" variant="outline" disabled={index === 0} onClick={() => setIndex(index - 1)}>
          Back
        </Button>
        {last ? (
          <Button size="sm" onClick={() => close(true)}>
            Done
          </Button>
        ) : (
          <Button size="sm" onClick={() => setIndex(index + 1)}>
            Next
          </Button>
        )}
        <Button size="sm" variant="ghost" className="ml-auto" onClick={() => close(true)}>
          Skip tour
        </Button>
      </div>
    </div>
    </>
  );
}
