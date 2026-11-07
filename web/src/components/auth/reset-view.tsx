"use client";

import { useMutation } from "@tanstack/react-query";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { resetPassword } from "@/lib/api/auth";
import { fieldErrors } from "@/lib/api/errors";

import { AuthCard, Notice } from "./auth-card";
import { FormError } from "./form-error";

export function ResetView() {
  const token = useSearchParams().get("token");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [mismatch, setMismatch] = useState(false);
  const m = useMutation({ mutationFn: () => resetPassword({ token: token!, password }) });
  const errs = m.error ? fieldErrors(m.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    const bad = password !== confirm;
    setMismatch(bad);
    if (!bad) m.mutate();
  }

  if (!token) {
    return (
      <AuthCard title="Reset your password">
        <FormError message="This reset link is incomplete. Request a new one." />
        <Link href="/auth/forgot" className="text-sm underline">
          Request a new link
        </Link>
      </AuthCard>
    );
  }
  if (m.isSuccess) {
    return (
      <AuthCard title="Password updated">
        <Notice>Your password has been changed and all sessions were signed out.</Notice>
        <Link href="/auth/login" className="text-sm underline">
          Sign in
        </Link>
      </AuthCard>
    );
  }
  return (
    <AuthCard title="Choose a new password">
      <form onSubmit={submit} noValidate className="space-y-4">
        <FormError error={m.error} />
        <Field id="password" label="New password" hint="12 to 256 characters." error={errs.password}>
          <input
            id="password"
            type="password"
            autoComplete="new-password"
            required
            className={inputClass}
            value={password}
            aria-invalid={!!errs.password}
            aria-describedby={errs.password ? "password-error" : undefined}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>
        <Field id="confirm" label="Confirm new password" error={mismatch ? "Passwords do not match." : undefined}>
          <input
            id="confirm"
            type="password"
            autoComplete="new-password"
            required
            className={inputClass}
            value={confirm}
            aria-invalid={mismatch}
            aria-describedby={mismatch ? "confirm-error" : undefined}
            onChange={(e) => setConfirm(e.target.value)}
          />
        </Field>
        <Button type="submit" className="w-full" disabled={m.isPending}>
          {m.isPending ? "Saving…" : "Change password"}
        </Button>
      </form>
    </AuthCard>
  );
}
