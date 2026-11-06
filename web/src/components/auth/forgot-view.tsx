"use client";

import { useMutation } from "@tanstack/react-query";
import Link from "next/link";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { forgotPassword } from "@/lib/api/auth";
import { fieldErrors } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";

import { AuthCard, LOG_MODE_NOTICE, Notice } from "./auth-card";
import { FormError } from "./form-error";

export function ForgotView() {
  const instance = useInstance();
  const [email, setEmail] = useState("");
  const m = useMutation({ mutationFn: () => forgotPassword(email.trim()) });
  const errs = m.error ? fieldErrors(m.error) : {};
  const logMode = instance.data?.email_delivery === "log";

  function submit(e: FormEvent) {
    e.preventDefault();
    m.mutate();
  }

  return (
    <AuthCard title="Reset your password">
      {m.isSuccess ? (
        <>
          <Notice>
            {logMode
              ? LOG_MODE_NOTICE
              : "If an account exists for that email, we have sent a link to reset the password."}
          </Notice>
        </>
      ) : (
        <form onSubmit={submit} noValidate className="space-y-4">
          {logMode ? <Notice>{LOG_MODE_NOTICE}</Notice> : null}
          <FormError error={m.error} />
          <Field id="email" label="Email" error={errs.email}>
            <input
              id="email"
              type="email"
              autoComplete="email"
              required
              className={inputClass}
              value={email}
              aria-invalid={!!errs.email}
              aria-describedby={errs.email ? "email-error" : undefined}
              onChange={(e) => setEmail(e.target.value)}
            />
          </Field>
          <Button type="submit" className="w-full" disabled={m.isPending}>
            {m.isPending ? "Sending…" : "Send reset link"}
          </Button>
        </form>
      )}
      <Link href="/auth/login" className="text-sm underline">
        Back to sign in
      </Link>
    </AuthCard>
  );
}
