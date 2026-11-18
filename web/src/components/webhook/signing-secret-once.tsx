"use client";

import { Button } from "@/components/ui/button";
import { CopyValue } from "@/components/ui/copy-value";
import { Notice } from "@/components/ui/notice";

/**
 * The signing secret, shown once (C-909, C-913). It lives only in the props of this component, which the caller holds in
 * state until the person confirms; closing the dialog or pressing the button drops it. It is never stored in a cache.
 */
export function SigningSecretOnce({ secret, onDone, doneLabel = "I have saved it" }: { secret: string; onDone: () => void; doneLabel?: string }) {
  return (
    <div className="space-y-3" data-testid="signing-secret-once">
      <Notice tone="pending" role="alert">
        <strong className="font-semibold">This will not be shown again.</strong> Copy the signing secret now and give it to the receiver, which uses it to check the{" "}
        <span className="font-mono">X-Aurora-Signature</span> header. If you lose it, make a new one by editing the webhook.
      </Notice>
      <CopyValue label="Signing secret" value={secret} />
      <Button type="button" onClick={onDone}>
        {doneLabel}
      </Button>
    </div>
  );
}
