"use client";

import { useMutation, useQueryClient, useQuery } from "@tanstack/react-query";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { useState, type FormEvent } from "react";

import { ErrorState, Loading } from "@/components/states";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { signup } from "@/lib/api/auth";
import { fieldErrors } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";
import { previewInvitation } from "@/lib/api/pending";
import { safeNext } from "@/lib/auth/use-unauthorized-redirect";

import { AuthCard, LOG_MODE_NOTICE, Notice } from "./auth-card";
import { FormError } from "./form-error";

export function SignupView() {
  const params = useSearchParams();
  const router = useRouter();
  const qc = useQueryClient();
  const invitation = params.get("invitation");
  const next = safeNext(params.get("next"));
  const instance = useInstance();
  const preview = useQuery({
    queryKey: ["invitation-preview", invitation],
    queryFn: () => previewInvitation(invitation!),
    enabled: !!invitation,
    retry: false,
  });
  const [displayName, setDisplayName] = useState("");
  const [emailInput, setEmailInput] = useState("");
  const [password, setPassword] = useState("");
  const email = preview.data?.email ?? emailInput;

  const m = useMutation({
    mutationFn: () =>
      signup({
        email: email.trim(),
        password,
        display_name: displayName.trim(),
        invitation_token: invitation,
      }),
    onSuccess: () => {
      qc.clear();
      router.replace(invitation ? `/join?token=${encodeURIComponent(invitation)}` : next);
    },
  });
  const errs = m.error ? fieldErrors(m.error) : {};

  if (instance.isPending) {
    return (
      <AuthCard title="Create your account">
        <Loading />
      </AuthCard>
    );
  }
  if (instance.isError) {
    return (
      <AuthCard title="Create your account">
        <ErrorState error={instance.error} onRetry={() => instance.refetch()} />
      </AuthCard>
    );
  }
  if (instance.data.signup_mode !== "open" && !invitation) {
    return (
      <AuthCard title="Sign-up is by invitation">
        <p className="text-sm">
          This instance is invite-only. Ask an organization administrator to invite you, then
          follow the link in the invitation email.
        </p>
        <Link href="/auth/login" className="text-sm underline">
          Back to sign in
        </Link>
      </AuthCard>
    );
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    m.mutate();
  }

  return (
    <AuthCard title="Create your account">
      {instance.data.email_delivery === "log" && !invitation ? (
        <Notice>{LOG_MODE_NOTICE}</Notice>
      ) : null}
      <form onSubmit={submit} noValidate className="space-y-4">
        <FormError error={m.error} />
        <Field id="display_name" label="Name" error={errs.display_name}>
          <input
            id="display_name"
            autoComplete="name"
            required
            className={inputClass}
            value={displayName}
            aria-invalid={!!errs.display_name}
            aria-describedby={errs.display_name ? "display_name-error" : undefined}
            onChange={(e) => setDisplayName(e.target.value)}
          />
        </Field>
        <Field id="email" label="Email" error={errs.email}>
          <input
            id="email"
            type="email"
            autoComplete="email"
            required
            className={inputClass}
            value={email}
            readOnly={!!preview.data}
            aria-invalid={!!errs.email}
            aria-describedby={errs.email ? "email-error" : undefined}
            onChange={(e) => setEmailInput(e.target.value)}
          />
        </Field>
        <Field
          id="password"
          label="Password"
          hint="12 to 256 characters."
          error={errs.password}
        >
          <input
            id="password"
            type="password"
            autoComplete="new-password"
            required
            minLength={12}
            className={inputClass}
            value={password}
            aria-invalid={!!errs.password}
            aria-describedby={errs.password ? "password-error" : undefined}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>
        <Button type="submit" className="w-full" disabled={m.isPending}>
          {m.isPending ? "Creating account…" : "Create account"}
        </Button>
      </form>
      <p className="text-sm">
        Already have an account?{" "}
        <Link href="/auth/login" className="underline">
          Sign in
        </Link>
      </p>
    </AuthCard>
  );
}
