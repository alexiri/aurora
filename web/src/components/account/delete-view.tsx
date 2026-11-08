"use client";

import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { DangerZone } from "@/components/ui/section";
import { deleteAccount } from "@/lib/api/account";
import { oidcStartUrl } from "@/lib/api/auth";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";

export function deleteErrorMessage(err: unknown): string {
  if (err instanceof ApiError && err.code === "last_owner") {
    return "You are the last owner of an organization. Transfer ownership or delete the organization first, then try again.";
  }
  if (err instanceof ApiError && err.code === "reauth_required") {
    return "Confirm it is you: sign in again with your provider, then come back and delete the account.";
  }
  return errorMessage(err);
}

/** Phase 1 (RFE-004): deleting the account needs an OIDC re-authentication within 10 minutes. */
export function DeleteView({ onDeleted }: { onDeleted?: () => void }) {
  const router = useRouter();
  const instance = useInstance();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const needsReauth = error instanceof ApiError && error.code === "reauth_required";
  const providers = instance.data?.oidc_providers ?? [];

  async function submit(e: FormEvent) {
    e.preventDefault();
    setPending(true);
    setError(null);
    try {
      await deleteAccount();
      (onDeleted ?? (() => router.push("/auth/login")))();
    } catch (err) {
      setError(err);
      setPending(false);
    }
  }

  return (
    <DangerZone
      id="delete-account"
      title="Delete account"
      description="This permanently deletes your account and signs you out everywhere. It cannot be undone. You will be asked to sign in again with your provider first."
    >
      <form onSubmit={(e) => void submit(e)} noValidate className="max-w-md space-y-4">
        {error !== null ? (
          <Notice tone="danger" role="alert">
            {deleteErrorMessage(error)}
          </Notice>
        ) : null}
        {needsReauth ? (
          <div className="space-y-2" aria-label="Re-authenticate">
            {providers.map((p) => (
              <Button key={p.id} asChild variant="outline" className="w-full">
                {/* Full navigation: the API redirects to the provider with prompt=login. */}
                <a href={oidcStartUrl(p.id, { reauth: true, returnTo: "/account/delete" })}>
                  Sign in again with {p.display_name}
                </a>
              </Button>
            ))}
          </div>
        ) : null}
        <Button type="submit" variant="danger" disabled={pending}>
          {pending ? "Deleting…" : "Delete my account"}
        </Button>
      </form>
    </DangerZone>
  );
}
