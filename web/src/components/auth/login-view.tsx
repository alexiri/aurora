"use client";

import { useSearchParams } from "next/navigation";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { oidcStartUrl } from "@/lib/api/auth";
import { ERROR_MESSAGES } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";
import { safeNext } from "@/lib/auth/use-unauthorized-redirect";

import { AuthCard, Notice } from "./auth-card";
import { FormError } from "./form-error";

/** The development provider (`auth.dev_idp`, docs/04): always labelled as such. */
const DEV_PROVIDER = "dev";

/**
 * Sign in (phase 1, RFE-004): OIDC provider buttons only. Errors from the callback arrive as
 * `?error=<code>` (signup_closed, email_unverified, user_disabled, ...).
 */
export function LoginView() {
  const params = useSearchParams();
  const next = safeNext(params.get("next"));
  const callbackCode = params.get("error");
  const instance = useInstance();
  const providers = instance.data?.oidc_providers ?? [];
  const hasDev = providers.some((p) => p.id === DEV_PROVIDER);

  return (
    <AuthCard title="Sign in">
      {hasDev ? (
        <Notice>
          <Badge tone="pending">Development</Badge> The development sign-in is enabled. It signs
          in without a password and must never be exposed.
        </Notice>
      ) : null}
      {callbackCode ? (
        <FormError
          message={ERROR_MESSAGES[callbackCode] ?? "Sign-in failed. Please try again."}
        />
      ) : null}
      {instance.isError ? (
        <FormError message="Could not load the sign-in options. Reload the page." />
      ) : null}
      {providers.length > 0 ? (
        <div className="space-y-2" aria-label="Sign-in providers">
          {providers.map((p) => (
            <Button key={p.id} asChild variant="outline" size="lg" className="w-full">
              {/* Full navigation to the API, which redirects to the provider. */}
              <a href={oidcStartUrl(p.id, { returnTo: next })}>
                {p.id === DEV_PROVIDER ? p.display_name : `Continue with ${p.display_name}`}
              </a>
            </Button>
          ))}
        </div>
      ) : instance.data ? (
        <FormError message="No sign-in provider is configured on this instance. Ask the operator to configure one." />
      ) : null}
    </AuthCard>
  );
}
