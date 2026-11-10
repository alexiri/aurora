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
export const TOUR_STOPS: readonly TourStop[] = [
  { target: "overview", title: "Overview", text: "Your agents, what they asked for and what they did. Start here each day." },
  { target: "agents", title: "Agents and Chat", text: "Your agents live here. Open one to chat with it, give it instructions or see its computer." },
  { target: "credentials", title: "Credentials", text: "The keys and logins your agents use, such as the LLM key. Come here when an agent needs to reach a service." },
  { target: "policy", title: "Policy", text: "The rules for what agents may do on the internet. Come here to allow or block something for good." },
  { target: "approvals", title: "Approvals", text: "When an agent wants to do something risky, it asks here. Look when a number appears." },
  { target: "activity", title: "Activity", text: "Every request your agents made, and what was decided." },
  { target: "members", title: "Members", text: "Who can use this organization. Come here to invite people." },
  { target: "settings", title: "Settings", text: "The workspace's name, time zone and limits." },
];

const NARROW = "(max-width: 767px)";

/** The navigation link a stop points at, if it is on screen (the sidebar and the drawer both carry one). */
function findTarget(target: string): HTMLElement | null {
  const all = document.querySelectorAll<HTMLElement>(`[data-tour="${target}"]`);
  for (const el of all) if (el.getClientRects().length > 0) return el;
  return null;
}

/**
 * Coach marks over the real navigation. Keyboard: Next (Enter or →), Back (←), Esc to leave. Focus moves to the card
 * at every stop and returns where it was at the end. No animation, so reduced motion needs nothing. At 390 px the
 * drawer opens as the tour goes and the card sits at the bottom of the screen.
 */
export function Tour({ setNavOpen }: { setNavOpen: (open: boolean) => void }) {
  const patch = usePatchOnboarding();
  const [stops, setStops] = useState<TourStop[] | null>(null);
  const [index, setIndex] = useState(0);
  const [narrow, setNarrow] = useState(false);
  const card = useRef<HTMLDivElement>(null);
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
    return () => el.removeAttribute("data-tour-active");
  }, [stop, narrow]);

  useEffect(() => {
    if (!stops) return;
    const last = stops.length - 1;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        close(true);
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
      <p className="text-xs font-semibold uppercase tracking-[0.06em] text-muted">
        Stop {index + 1} of {stops.length}
      </p>
      <h2 id="tour-title" className="mt-1 font-display text-base font-semibold">
        {stop.title}
      </h2>
      <p id="tour-text" className="mt-1 text-sm text-ink-2">
        {stop.text}
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
  );
}
