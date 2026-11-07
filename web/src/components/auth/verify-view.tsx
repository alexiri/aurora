"use client";

import { useMutation } from "@tanstack/react-query";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useEffect, useRef } from "react";

import { Loading } from "@/components/states";
import { verifyEmail } from "@/lib/api/auth";

import { AuthCard, Notice } from "./auth-card";
import { FormError } from "./form-error";
import { ResendButton } from "./resend-button";

export function VerifyView() {
  const token = useSearchParams().get("token");
  const started = useRef(false);
  const m = useMutation({ mutationFn: (t: string) => verifyEmail(t) });
  const { mutate } = m;

  useEffect(() => {
    if (token && !started.current) {
      started.current = true;
      mutate(token);
    }
  }, [token, mutate]);

  return (
    <AuthCard title="Verify your email">
      {!token ? (
        <FormError message="This verification link is incomplete." />
      ) : m.isSuccess ? (
        <>
          <Notice>Your email address is verified.</Notice>
          <Link href="/" className="text-sm underline">
            Continue
          </Link>
        </>
      ) : m.isError ? (
        <>
          <FormError error={m.error} />
          <ResendButton />
        </>
      ) : (
        <Loading label="Verifying…" />
      )}
    </AuthCard>
  );
}
