"use client";

import { useState, type FormEvent } from "react";

import { errorMessage, fieldErrors } from "@/lib/api/errors";
import type { CreateWorkspaceRequest } from "@/lib/api/queries";
import { MEMORY_LANGUAGES, timeZones } from "@/lib/nav/landing";
import { validateNameSlug } from "@/lib/nav/validation";

import { Button } from "./ui/button";
import { Field, inputClass } from "./ui/field";

const ZONES = timeZones();

export function CreateWorkspaceForm({
  onSubmit,
  pending,
  error,
  defaultTimezone,
}: {
  onSubmit: (body: CreateWorkspaceRequest) => void;
  pending: boolean;
  error: unknown;
  defaultTimezone?: string;
}) {
  const browserZone = defaultTimezone ?? Intl.DateTimeFormat().resolvedOptions().timeZone;
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [timezone, setTimezone] = useState(ZONES.includes(browserZone) ? browserZone : "UTC");
  const [memoryLanguage, setMemoryLanguage] = useState("english");
  const [clientErrors, setClientErrors] = useState<Record<string, string>>({});

  const serverErrors = error ? fieldErrors(error) : {};
  const errs: Record<string, string | undefined> = { ...serverErrors };
  for (const [k, v] of Object.entries(clientErrors)) if (v) errs[k] = v;
  const hasFieldErrors = Object.values(serverErrors).some(Boolean);

  function submit(e: FormEvent) {
    e.preventDefault();
    const v = validateNameSlug({ name, slug });
    setClientErrors(v);
    if (Object.keys(v).length > 0) return;
    onSubmit({
      name: name.trim(),
      slug: slug.trim() || null,
      timezone,
      memory_language: memoryLanguage,
    });
  }

  return (
    <form onSubmit={submit} noValidate className="space-y-4">
      <Field id="ws-name" label="Name" error={errs.name}>
        <input
          id="ws-name"
          className={inputClass}
          value={name}
          maxLength={200}
          autoComplete="off"
          aria-invalid={!!errs.name}
          aria-describedby={errs.name ? "ws-name-error" : undefined}
          onChange={(e) => {
            setName(e.target.value);
            setClientErrors((c) => ({ ...c, name: "" }));
          }}
        />
      </Field>
      <Field
        id="ws-slug"
        label="URL name (optional)"
        hint="Derived from the name when empty."
        error={errs.slug}
      >
        <input
          id="ws-slug"
          className={inputClass}
          value={slug}
          autoComplete="off"
          aria-invalid={!!errs.slug}
          aria-describedby={errs.slug ? "ws-slug-error" : undefined}
          onChange={(e) => {
            setSlug(e.target.value);
            setClientErrors((c) => ({ ...c, slug: "" }));
          }}
        />
      </Field>
      <Field id="ws-tz" label="Time zone" error={errs.timezone}>
        <select
          id="ws-tz"
          className={inputClass}
          value={timezone}
          onChange={(e) => setTimezone(e.target.value)}
        >
          {ZONES.map((z) => (
            <option key={z} value={z}>
              {z}
            </option>
          ))}
        </select>
      </Field>
      <Field id="ws-lang" label="Memory language" error={errs.memory_language}>
        <select
          id="ws-lang"
          className={inputClass}
          value={memoryLanguage}
          onChange={(e) => setMemoryLanguage(e.target.value)}
        >
          {MEMORY_LANGUAGES.map((l) => (
            <option key={l} value={l}>
              {l}
            </option>
          ))}
        </select>
      </Field>
      {error && !hasFieldErrors ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(error)}
        </p>
      ) : null}
      <div className="flex justify-end">
        <Button type="submit" disabled={pending}>
          {pending ? "Creating…" : "Create workspace"}
        </Button>
      </div>
    </form>
  );
}
