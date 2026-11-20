"use client";

import { useId, useState, useSyncExternalStore, type ReactNode } from "react";

import { ALTITUDE_REGISTRY, isAltitudeId, type AltitudeId } from "@/lib/altitude-registry";
import {
  AltitudeScope,
  dismissHint,
  HINT_AFTER_OPENED,
  hintDismissed,
  noteOpened,
  openedCount,
  useAltitude,
  useHiddenControls,
  useHiddenCount,
} from "@/lib/altitude";
import { useMoreControls } from "@/lib/api/use-more-controls";

import { Button } from "./button";
import { Notice } from "./notice";

export { AltitudeScope };

const OPENED_EVENT = "aurora:altitude-opened";

/**
 * (P5-REFINE-3, D-406) Wraps one control by its altitude. Shown when the person's floor or "Show more controls" switch
 * reaches `level` and `allowed` (the role cap: false stays hidden at any altitude). Otherwise it adds one hidden
 * control to the page's `AltitudeScope`, and, with a `label`, draws the collapsed line in its place: the line opens
 * just this control in place; the page footer (`MoreControlsLine`) turns the switch on for everything.
 *
 * `promote` shows the children at any altitude: contextual promotion (a chat error about the AI key shows the key fix;
 * a failed webhook test shows the receiver's status). The caller passes the condition.
 *
 * `level` must match the registry row of `id` (`lib/altitude-registry.ts`); `tests/altitude-registry.test.ts` checks it.
 */
export function Altitude({
  id,
  level,
  allowed = true,
  label,
  promote = false,
  children,
}: {
  id: AltitudeId;
  level: "setup" | "expert";
  allowed?: boolean;
  label?: string;
  promote?: boolean;
  children: ReactNode;
}) {
  const known = isAltitudeId(id);
  if (!known && process.env.NODE_ENV === "test") throw new Error(`Altitude: unknown id "${id}" (add it to lib/altitude-registry.ts)`);
  const alt = useAltitude();
  const [openedHere, setOpenedHere] = useState(false);
  const shown = !known || promote || (allowed && (alt.allows(level) || openedHere));
  const hidden = known && !shown && allowed;
  // A control the role never allows is not "hidden": the person could not turn it on.
  useHiddenControls(hidden && !openedHere ? 1 : 0);
  const labelId = useId();
  if (shown) return <>{children}</>;
  if (!allowed || !label) return null;
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 rounded-control border border-dashed border-line px-3 py-2 text-sm text-muted" data-altitude={id}>
      <span id={labelId}>{label}</span>
      <button
        type="button"
        aria-describedby={labelId}
        className="min-h-9 text-accent-ink underline underline-offset-2 hover:no-underline"
        onClick={() => {
          setOpenedHere(true);
          noteOpened();
          window.dispatchEvent(new Event(OPENED_EVENT));
        }}
      >
        Show
      </button>
    </div>
  );
}

/** The registry's level of an id (for code that classifies without wrapping, like the agent page list). */
export function altitudeOf(id: AltitudeId): "use" | "setup" | "expert" {
  return ALTITUDE_REGISTRY[id].altitude;
}

/**
 * The page's footer: "3 more controls hidden · Show" (turns the per-person switch on), and "Showing more controls ·
 * Hide" while it is on. Nothing when nothing is hidden and the switch is off. Also carries the one-time hint after
 * three collapsed lines were opened in place.
 */
export function MoreControlsLine({ className }: { className?: string }) {
  const { more, setMore, pending, error } = useMoreControls();
  const hiddenCount = useHiddenCount();
  const hint = useHint(more);
  if (!more && hiddenCount === 0 && !hint.visible) return null;
  return (
    <div className={className} data-more-controls-line>
      {hint.visible ? (
        <Notice role="status" className="mb-2 flex flex-wrap items-center gap-2">
          <span>Want everything? Turn on Show more controls.</span>
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              hint.dismiss();
              setMore(true);
            }}
          >
            Turn on
          </Button>
          <Button size="sm" variant="ghost" onClick={hint.dismiss}>
            Dismiss
          </Button>
        </Notice>
      ) : null}
      {more || hiddenCount > 0 ? (
        <p className="text-xs text-muted">
          {more ? "Showing more controls" : `${hiddenCount} more ${hiddenCount === 1 ? "control" : "controls"} hidden`}
          {" · "}
          <button
            type="button"
            disabled={pending}
            className="min-h-9 text-accent-ink underline underline-offset-2 hover:no-underline disabled:opacity-50"
            onClick={() => setMore(!more)}
          >
            {more ? "Hide" : "Show"}
          </button>
        </p>
      ) : null}
      {error ? (
        <p role="alert" className="text-xs text-danger-ink">
          {error}
        </p>
      ) : null}
    </div>
  );
}

const HINT_EVENT = "aurora:altitude-hint";

function subscribe(on: () => void) {
  window.addEventListener(OPENED_EVENT, on);
  window.addEventListener(HINT_EVENT, on);
  return () => {
    window.removeEventListener(OPENED_EVENT, on);
    window.removeEventListener(HINT_EVENT, on);
  };
}

/** The one-time hint: the counters live in `localStorage`; the server render and a missing storage read "never". */
function useHint(more: boolean) {
  const opened = useSyncExternalStore(subscribe, openedCount, () => 0);
  const dismissed = useSyncExternalStore(subscribe, hintDismissed, () => true);
  return {
    visible: !more && !dismissed && opened >= HINT_AFTER_OPENED,
    dismiss: () => {
      dismissHint();
      window.dispatchEvent(new Event(HINT_EVENT));
    },
  };
}
