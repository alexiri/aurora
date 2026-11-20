"use client";

import { useEffect, useRef, useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { CheckIcon } from "@/components/ui/icons";
import { Dialog, DialogContent } from "@/components/ui/dialog";

/**
 * (P5-REFINE-2) The copy state of a one-time secret: `used` becomes true with the first successful copy and stays true,
 * `state` is the button's own ("copied" for two seconds, "failed" when the browser refused). The state lives with the
 * component that shows the secret, so it starts over with the next secret.
 */
export function useSecretCopy(secret: string | null) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const [used, setUsed] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );
  function copy() {
    if (secret === null) return;
    let write: Promise<void> | undefined;
    try {
      write = navigator.clipboard?.writeText(secret);
    } catch {
      write = undefined;
    }
    if (!write) return setState("failed");
    write.then(
      () => {
        setUsed(true);
        setState("copied");
        if (timer.current) clearTimeout(timer.current);
        timer.current = setTimeout(() => setState("idle"), 2000);
      },
      () => setState("failed"),
    );
  }
  return { used, state, copy };
}

/**
 * The secret in a scrollable mono box that never wraps, with **Copy** on its right; Copy turns into **Copied** and stays
 * so once used. `data-testid="secret-key"` is on the box.
 */
export function SecretBox({
  secret,
  used,
  state,
  onCopy,
  copyLabel = "Copy",
}: {
  secret: string;
  used: boolean;
  state: "idle" | "copied" | "failed";
  onCopy: () => void;
  copyLabel?: string;
}) {
  return (
    <div>
      <div className="flex min-w-0 items-center gap-2">
        <code
          data-testid="secret-key"
          tabIndex={0}
          aria-label="Secret"
          className="block min-w-0 flex-1 select-all overflow-x-auto whitespace-nowrap rounded-control border border-line bg-subtle p-3 font-mono text-sm text-ink"
        >
          {secret}
        </code>
        <Button type="button" variant="outline" onClick={onCopy} aria-label={used ? "Copied" : copyLabel} className="shrink-0">
          {used ? (
            <>
              <CheckIcon aria-hidden="true" className="size-4" />
              Copied
            </>
          ) : (
            copyLabel
          )}
        </Button>
      </div>
      <p role="status" aria-live="polite" className={state === "failed" ? "mt-1 text-xs text-danger-ink" : "sr-only"}>
        {state === "failed" ? "Your browser blocked copying; select the text instead." : used ? "Copied" : ""}
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
}: {
  secret: string;
  title: string;
  description?: string;
  children?: ReactNode;
  onClose: () => void;
  copyLabel?: string;
  doneLabel?: string;
}) {
  const { used, state, copy } = useSecretCopy(secret);
  const [asking, setAsking] = useState(false);
  const tryClose = () => (used ? onClose() : setAsking(true));
  return (
    <Dialog open onOpenChange={(open) => (open ? undefined : tryClose())}>
      <DialogContent title={title} description={description}>
        <div className="space-y-4">
          <SecretBox secret={secret} used={used} state={state} onCopy={copy} copyLabel={copyLabel} />
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
