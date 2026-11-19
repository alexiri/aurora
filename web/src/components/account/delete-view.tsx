"use client";

import { useRouter } from "next/navigation";
import type { FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { DangerZone } from "@/components/ui/section";
import { deleteAccount } from "@/lib/api/account";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useAsyncAction } from "@/lib/use-async-action";

export function deleteErrorMessage(err: unknown): string {
  if (err instanceof ApiError && err.code === "last_owner") {
    return "You are the last owner of an organization. Transfer ownership or delete the organization first, then try again.";
  }
  return errorMessage(err);
}

/** Phase 1 (RFE-004): delete the account. No re-authentication step (phase 5, FIX-157, D-317). */
export function DeleteView({ onDeleted }: { onDeleted?: () => void }) {
  const router = useRouter();
  const { pending, error, run } = useAsyncAction(deleteAccount, onDeleted ?? (() => router.push("/auth/login")));
  function submit(e: FormEvent) {
    e.preventDefault();
    void run();
  }

  return (
    <DangerZone
      id="delete-account"
      title="Delete account"
      description="This permanently deletes your account and signs you out everywhere. It cannot be undone."
    >
      <form onSubmit={submit} noValidate className="max-w-md space-y-4">
        {error !== null ? (
          <Notice tone="danger" role="alert">
            {deleteErrorMessage(error)}
          </Notice>
        ) : null}
        <Button type="submit" variant="danger" disabled={pending}>
          {pending ? "Deleting…" : "Delete my account"}
        </Button>
      </form>
    </DangerZone>
  );
}
