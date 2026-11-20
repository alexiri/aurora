"use client";

import { useState, type FormEvent } from "react";

import { fieldErrors } from "@/lib/api/errors";
import type { CreateWorkspaceRequest } from "@/lib/api/queries";
import { MEMORY_LANGUAGES, timeZones } from "@/lib/nav/landing";
import { validateNameSlug } from "@/lib/nav/validation";

import { Altitude } from "./ui/altitude";
import { Button } from "./ui/button";
import { Field, inputClass } from "./ui/field";
import { ErrorNotice } from "./ui/notice";
import { Select } from "@/components/ui/select";

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
        label="Web address (optional)"
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
        <Select id="ws-tz" value={timezone} onChange={setTimezone} options={ZONES.map((z) => ({ value: z, label: z }))} />
      </Field>
      <Altitude id="ws.memlang" level="expert" promote={!!errs.memory_language}>
        <Field id="ws-lang" label="Language agents remember in" error={errs.memory_language}>
          <Select id="ws-lang" value={memoryLanguage} onChange={setMemoryLanguage} options={MEMORY_LANGUAGES.map((l) => ({ value: l, label: l }))} />
        </Field>
      </Altitude>
      {error && !hasFieldErrors ? (
        <ErrorNotice error={error} />
      ) : null}
      <div className="flex justify-end">
        <Button type="submit" size="lg" disabled={pending}>
          {pending ? "Creating…" : "Create workspace"}
        </Button>
      </div>
    </form>
  );
}
