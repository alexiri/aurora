"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { CloseAnywayConfirm, SecretBox, useSecretCopy } from "@/components/ui/secret-once";

/**
 * The signing secret, shown once (C-909, C-913). It lives only in the props of this component, which the caller holds in
 * state until the person confirms; closing the dialog or pressing the button drops it. It is never stored in a cache.
 * (P5-REFINE-2) It is a `SecretOnce` body: the same scrollable box, Copy → Copied, and "You haven't copied it. Close
 * anyway?" before the button closes without a copy.
 */
export function SigningSecretOnce({ secret, onDone, doneLabel = "I have saved it" }: { secret: string; onDone: () => void; doneLabel?: string }) {
  const { used, state, copy } = useSecretCopy(secret);
  const [asking, setAsking] = useState(false);
  return (
    <div className="space-y-3" data-testid="signing-secret-once">
      <Notice tone="pending" role="alert">
        <strong className="font-semibold">This will not be shown again.</strong> Copy the signing secret now and give it to the receiver, which uses it to check the{" "}
        <span className="font-mono">X-Aurora-Signature</span> header. If you lose it, make a new one by editing the webhook.
      </Notice>
      <SecretBox secret={secret} used={used} state={state} onCopy={copy} copyLabel="Copy" />
      {asking && !used ? <CloseAnywayConfirm onCopy={copy} onClose={onDone} /> : null}
      <Button type="button" onClick={() => (used ? onDone() : setAsking(true))}>
        {doneLabel}
      </Button>
    </div>
  );
}
