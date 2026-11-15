"use client";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { oidcStartUrl } from "@/lib/api/auth";
import { useInstance } from "@/lib/api/queries";

/**
 * The re-authentication step of a settings write (SEC-90): the API answers 412 `reauth_required`
 * when the session's last sign-in is older than 10 minutes; the admin signs in again with a provider
 * (full navigation, `prompt=login`) and comes back to `returnTo`, then retries.
 */
export function ReauthPrompt({ returnTo, onRetry }: { returnTo: string; onRetry?: () => void }) {
  const instance = useInstance();
  const providers = instance.data?.oidc_providers ?? [];
  return (
    <Notice tone="pending" role="alert" className="space-y-3">
      <p>Confirm it is you: sign in again with your provider, then come back and save again. Changes to instance settings need a sign-in from the last 10 minutes.</p>
      <div className="flex flex-wrap gap-2" aria-label="Re-authenticate">
        {providers.map((p) => (
          <Button key={p.id} asChild variant="outline" size="sm">
            <a href={oidcStartUrl(p.id, { reauth: true, returnTo })}>Sign in again with {p.display_name}</a>
          </Button>
        ))}
        {onRetry ? (
          <Button type="button" size="sm" onClick={onRetry}>
            I have signed in again — retry
          </Button>
        ) : null}
      </div>
    </Notice>
  );
}
