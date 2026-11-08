"use client";

import { useState, type FormEvent } from "react";

import { ApiError } from "@/lib/api/client";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import type { CreateOrgRequest } from "@/lib/api/queries";
import { validateNameSlug } from "@/lib/nav/validation";

import { Button } from "./ui/button";
import { Field, inputClass } from "./ui/field";
import { Notice } from "./ui/notice";

export function CreateOrgForm({
  onSubmit,
  pending,
  error,
}: {
  onSubmit: (body: CreateOrgRequest) => void;
  pending: boolean;
  error: unknown;
}) {
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});
  const server = error ? fieldErrors(error) : {};
  const errs: Record<string, string | undefined> = { ...server };
  for (const [k, v] of Object.entries(clientErrors)) if (v) errs[k] = v;
  const forbidden = error instanceof ApiError && error.code === "org_creation_forbidden";

  function submit(e: FormEvent) {
    e.preventDefault();
    const v = validateNameSlug({ name, slug });
    setClientErrors(v);
    if (Object.keys(v).length > 0) return;
    onSubmit({ name: name.trim(), slug: slug.trim() || null });
  }

  return (
    <form onSubmit={submit} noValidate className="mx-auto w-full max-w-sm space-y-4 text-left">
      <Field id="org-name" label="Organization name" error={errs.name}>
        <input
          id="org-name"
          className={inputClass}
          value={name}
          autoComplete="off"
          aria-invalid={!!errs.name}
          aria-describedby={errs.name ? "org-name-error" : undefined}
          onChange={(e) => setName(e.target.value)}
        />
      </Field>
      <Field id="org-slug" label="URL name (optional)" error={errs.slug}>
        <input
          id="org-slug"
          className={inputClass}
          value={slug}
          autoComplete="off"
          aria-invalid={!!errs.slug}
          aria-describedby={errs.slug ? "org-slug-error" : undefined}
          onChange={(e) => setSlug(e.target.value)}
        />
      </Field>
      {error && (forbidden || !Object.values(server).some(Boolean)) ? (
        <Notice tone="danger" role="alert">
          {errorMessage(error)}
        </Notice>
      ) : null}
      <Button type="submit" size="lg" disabled={pending}>
        {pending ? "Creating…" : "Create organization"}
      </Button>
    </form>
  );
}
