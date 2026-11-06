"use client";

import { useMutation, useQueryClient } from "@tanstack/react-query";
import Link from "next/link";
import { useRouter, useSearchParams } from "next/navigation";
import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { login } from "@/lib/api/auth";
import { ERROR_MESSAGES, fieldErrors } from "@/lib/api/errors";
import { useInstance } from "@/lib/api/queries";
import { safeNext } from "@/lib/auth/use-unauthorized-redirect";

import { AuthCard } from "./auth-card";
import { FormError } from "./form-error";

export function LoginView() {
  const params = useSearchParams();
  const router = useRouter();
  const qc = useQueryClient();
  const next = safeNext(params.get("next"));
  const callbackCode = params.get("error");
  const instance = useInstance();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");

  const m = useMutation({
    mutationFn: () => login({ email: email.trim(), password }),
    onSuccess: () => {
      qc.clear();
      router.replace(next);
    },
  });
  const errs = m.error ? fieldErrors(m.error) : {};
  const providers = instance.data?.oidc_providers ?? [];
  const signupVisible = instance.data ? instance.data.signup_mode !== "invite_only" : false;

  function submit(e: FormEvent) {
    e.preventDefault();
    m.mutate();
  }

  return (
    <AuthCard title="Sign in">
      {callbackCode ? (
        <FormError
          message={ERROR_MESSAGES[callbackCode] ?? "Sign-in failed. Please try again."}
        />
      ) : null}
      <form onSubmit={submit} noValidate className="space-y-4">
        <FormError error={m.error} />
        <Field id="email" label="Email" error={errs.email}>
          <input
            id="email"
            type="email"
            autoComplete="username"
            required
            className={inputClass}
            value={email}
            aria-invalid={!!errs.email}
            aria-describedby={errs.email ? "email-error" : undefined}
            onChange={(e) => setEmail(e.target.value)}
          />
        </Field>
        <Field id="password" label="Password" error={errs.password}>
          <input
            id="password"
            type="password"
            autoComplete="current-password"
            required
            className={inputClass}
            value={password}
            aria-invalid={!!errs.password}
            aria-describedby={errs.password ? "password-error" : undefined}
            onChange={(e) => setPassword(e.target.value)}
          />
        </Field>
        <Button type="submit" className="w-full" disabled={m.isPending}>
          {m.isPending ? "Signing in…" : "Sign in"}
        </Button>
      </form>
      {providers.length > 0 ? (
        <div className="space-y-2" aria-label="Single sign-on">
          {providers.map((p) => (
            <Button key={p.id} asChild variant="outline" className="w-full">
              {/* Full navigation to the API, which redirects to the provider. */}
              <a
                href={`/v1/auth/oidc/${encodeURIComponent(p.id)}/start?return_to=${encodeURIComponent(next)}`}
              >
                Continue with {p.display_name}
              </a>
            </Button>
          ))}
        </div>
      ) : null}
      <p className="text-sm">
        <Link href="/auth/forgot" className="underline">
          Forgot your password?
        </Link>
        {signupVisible ? (
          <>
            {" · "}
            <Link href="/auth/signup" className="underline">
              Create an account
            </Link>
          </>
        ) : null}
      </p>
    </AuthCard>
  );
}
