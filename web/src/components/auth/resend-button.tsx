"use client";

import { useMutation } from "@tanstack/react-query";

import { Button } from "@/components/ui/button";
import { resendVerification } from "@/lib/api/auth";
import { ApiError } from "@/lib/api/client";
import { errorMessage } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";

import { LOG_MODE_NOTICE, Notice } from "./auth-card";
import { FormError } from "./form-error";

/** Resend the verification email (requires a signed-in session). */
export function ResendButton() {
  const instance = useInstance();
  const m = useMutation({ mutationFn: resendVerification });
  if (m.isSuccess) {
    return (
      <Notice>
        {instance.data?.email_delivery === "log"
          ? LOG_MODE_NOTICE
          : "Verification email sent. Check your inbox."}
      </Notice>
    );
  }
  const unauth = m.error instanceof ApiError && m.error.status === 401;
  return (
    <div className="space-y-2">
      <FormError
        message={
          m.error
            ? unauth
              ? "Sign in first, then request a new verification email."
              : errorMessage(m.error)
            : undefined
        }
      />
      <Button type="button" variant="outline" disabled={m.isPending} onClick={() => m.mutate()}>
        {m.isPending ? "Sending…" : "Resend verification email"}
      </Button>
    </div>
  );
}
