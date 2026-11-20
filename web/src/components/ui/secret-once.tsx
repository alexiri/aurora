"use client";

import { useEffect, useState, type ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { CheckIcon } from "@/components/ui/icons";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { useCopy } from "@/lib/use-copy";

/**
 * (P5-REFINE-2) The copy state of a one-time secret: `copied` becomes true with the first successful copy and stays true,
 * `state` is the button's own ("copied" for two seconds, "failed" when the browser refused).
 */
export function useSecretCopy(secret: string | null) {
  const { state, copy } = useCopy();
  const [used, setUsed] = useState(false);
  useEffect(() => {
    if (state === "copied") setUsed(true);
  }, [state]);
  useEffect(() => {
    if (secret === null) setUsed(false);
  }, [secret]);
  return { used, state, copy: () => secret !== null && copy(secret) };
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
export function CloseAnywayConfirm({ onCopy, onClose, onKeep }: { onCopy: () => void; onClose: () => void; onKeep?: () => void }) {
  return (
    <div role="alert" className="space-y-2 rounded-control border border-pending-line bg-pending-panel p-3 text-sm text-pending-ink">
      <p className="font-medium">You haven&apos;t copied it. Close anyway?</p>
      <div className="flex flex-wrap gap-2">
        <Button type="button" size="sm" onClick={() => (onKeep?.(), onCopy())}>
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
  title,
  description,
  children,
  onClose,
  copyLabel,
  doneLabel = "I have saved it",
}: {
  secret: string | null;
  title: string;
  description?: string;
  children?: ReactNode;
  onClose: () => void;
  copyLabel?: string;
  doneLabel?: string;
}) {
  const { used, state, copy } = useSecretCopy(secret);
  const [asking, setAsking] = useState(false);
  useEffect(() => {
    if (secret === null) setAsking(false);
  }, [secret]);
  const close = () => {
    setAsking(false);
    onClose();
  };
  const tryClose = () => (used ? close() : setAsking(true));
  return (
    <Dialog open={secret !== null} onOpenChange={(open) => (open ? undefined : tryClose())}>
      <DialogContent title={title} description={description}>
        {secret !== null ? (
          <div className="space-y-4">
            <SecretBox secret={secret} used={used} state={state} onCopy={copy} copyLabel={copyLabel} />
            {children}
            {asking && !used ? <CloseAnywayConfirm onCopy={copy} onClose={close} /> : null}
            <div className="flex justify-end">
              <Button type="button" onClick={tryClose}>
                {doneLabel}
              </Button>
            </div>
          </div>
        ) : null}
      </DialogContent>
    </Dialog>
  );
}
