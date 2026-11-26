"use client";

import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";

import { Button } from "@/components/ui/button";
import { CheckIcon } from "@/components/ui/icons";
import { Dialog, DialogContent, type DialogSize } from "@/components/ui/dialog";

/**
 * (P5-REFINE-2, FIX-248) The copy state of a one-time secret: `used` becomes true with the first successful copy and
 * stays true (it only gates the "You haven't copied it. Close anyway?" confirm), `state` is the button's own ("copied"
 * for two seconds, "failed" when the browser refused), `copies` counts the successful copies (so the screen-reader line
 * can word every one differently). `boxRef` sits on the secret's box, for the select-and-execCommand fallback. The state
 * lives with the component that shows the secret, so it starts over with the next secret.
 */
export function useSecretCopy(secret: string | null) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const [used, setUsed] = useState(false);
  const [copies, setCopies] = useState(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const boxRef = useRef<HTMLCodeElement | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  function copy() {
    if (secret === null) return;
    void writeToClipboard(secret, boxRef.current).then((ok) => {
      if (!ok) return setState("failed");
      setUsed(true);
      setCopies((n) => n + 1);
      setState("copied");
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setState("idle"), 2000);
    });
  }
  return { used, state, copies, copy, boxRef };
}

/**
 * The clipboard write. `navigator.clipboard.writeText` can be missing or refuse silently inside a dialog (a focus trap,
 * an iframe, a non-secure context), so a refusal — or no async clipboard at all — falls back to selecting the box's text
 * and the deprecated-but-ubiquitous `document.execCommand("copy")`.
 */
async function writeToClipboard(text: string, box: HTMLElement | null): Promise<boolean> {
  let write: Promise<void> | undefined;
  try {
    write = navigator.clipboard?.writeText(text);
  } catch {
    write = undefined;
  }
  if (write) {
    try {
      await write;
      return true;
    } catch {
      // fall through to the fallback
    }
  }
  if (!box || typeof document.execCommand !== "function") return false;
  const selection = window.getSelection();
  if (!selection) return false;
  const range = document.createRange();
  range.selectNodeContents(box);
  selection.removeAllRanges();
  selection.addRange(range);
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  selection.removeAllRanges();
  return ok;
}

/** The screen-reader line's words after a copy: worded differently each time, because a repeated identical text is not announced. */
function copiedWords(copies: number): string {
  if (copies <= 1) return "Copied";
  if (copies === 2) return "Copied again.";
  return `Copied ${copies} times.`;
}

/**
 * The secret in a scrollable mono box that never wraps, with **Copy** on its right; after a click it says **Copied** for
 * about two seconds, then goes back to **Copy again** — every click copies and gives that feedback (FIX-248), and a
 * refused copy keeps the button on Copy and says so in red. `data-testid="secret-key"` is on the box.
 */
export function SecretBox({
  secret,
  used,
  state,
  onCopy,
  copyLabel = "Copy",
  copies = 0,
  boxRef,
}: {
  secret: string;
  used: boolean;
  state: "idle" | "copied" | "failed";
  onCopy: () => void;
  copyLabel?: string;
  /** How many copies have succeeded, for the screen-reader line's wording. */
  copies?: number;
  /** Ref on the box, for the select-and-execCommand copy fallback. */
  boxRef?: RefObject<HTMLCodeElement | null>;
}) {
  const label = state === "copied" ? "Copied" : used ? "Copy again" : copyLabel;
  return (
    <div>
      <div className="flex min-w-0 items-center gap-2">
        <code
          ref={boxRef}
          data-testid="secret-key"
          tabIndex={0}
          aria-label="Value shown once"
          className="block min-w-0 flex-1 select-all overflow-x-auto whitespace-nowrap rounded-control border border-line bg-subtle p-3 font-mono text-sm text-ink"
        >
          {secret}
        </code>
        <Button type="button" variant="outline" onClick={onCopy} aria-label={label} className="shrink-0">
          {state === "copied" ? (
            <>
              <CheckIcon aria-hidden="true" className="size-4" />
              Copied
            </>
          ) : (
            label
          )}
        </Button>
      </div>
      <p role="status" aria-live="polite" className={state === "failed" ? "mt-1 text-xs text-danger-ink" : "sr-only"}>
        {state === "failed" ? "Your browser blocked copying; select the text instead." : copies > 0 ? copiedWords(copies) : ""}
      </p>
    </div>
  );
}

/** "You haven't copied it. Close anyway?" with **Copy** and **Close anyway**. */
export function CloseAnywayConfirm({ onCopy, onClose }: { onCopy: () => void; onClose: () => void }) {
  return (
    <div role="alert" className="space-y-2 rounded-control border border-pending-line bg-pending-panel p-3 text-sm text-pending-ink">
      <p className="font-medium">You haven&apos;t copied it. Close anyway?</p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" size="sm" onClick={onCopy}>
          Copy
        </Button>
        <Button type="button" size="sm" variant="outline" onClick={onClose}>
          Close anyway
        </Button>
      </div>
    </div>
  );
}

/**
 * (P5-REFINE-2, theme "One-time secrets") Shows a secret exactly once: an API key, a token, a signing secret or a webhook
 * URL. The opener owns the value in component state only and clears it in `onClose`; it is never written to storage,
 * logs or the URL. Closing (the X, Escape, the overlay or the button) before the secret was copied asks "You haven't
 * copied it. Close anyway?"; after it was copied, closing is immediate.
 */
export function SecretOnce({
  secret,
  ...rest
}: {
  secret: string | null;
  title: string;
  description?: string;
  children?: ReactNode;
  onClose: () => void;
  copyLabel?: string;
  doneLabel?: string;
  /** The dialog's width (`lg` = 640 px for a value and a panel beside it). */
  size?: DialogSize;
  /** A heading above the box ("Your address", "Your secret"). */
  label?: string;
}) {
  // Mounted only while there is a secret, so copy and ask state start over with every new one.
  return secret === null ? null : <OpenSecret secret={secret} {...rest} />;
}

function OpenSecret({
  secret,
  title,
  description,
  children,
  onClose,
  copyLabel,
  doneLabel = "I have saved it",
  size,
  label,
}: {
  secret: string;
  title: string;
  description?: string;
  children?: ReactNode;
  onClose: () => void;
  copyLabel?: string;
  doneLabel?: string;
  size?: DialogSize;
  label?: string;
}) {
  const { used, state, copies, copy, boxRef } = useSecretCopy(secret);
  const [asking, setAsking] = useState(false);
  const tryClose = () => (used ? onClose() : setAsking(true));
  return (
    <Dialog open onOpenChange={(open) => (open ? undefined : tryClose())}>
      <DialogContent title={title} description={description} size={size}>
        <div className="space-y-4">
          {label ? <p className="text-sm font-medium text-ink">{label}</p> : null}
          <SecretBox secret={secret} used={used} state={state} onCopy={copy} copyLabel={copyLabel} copies={copies} boxRef={boxRef} />
          {children}
          {asking && !used ? <CloseAnywayConfirm onCopy={copy} onClose={onClose} /> : null}
          <div className="flex justify-end">
            <Button type="button" onClick={tryClose}>
              {doneLabel}
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
