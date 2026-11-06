"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { usePatchWorkspace } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import type { Workspace } from "@/lib/api/queries";
import { MEMORY_LANGUAGES, timeZones } from "@/lib/nav/landing";

export function GeneralTab({ workspace, canOwn }: { workspace: Workspace; canOwn: boolean }) {
  const patch = usePatchWorkspace(workspace.id);
  const zones = timeZones();
  const [name, setName] = useState(workspace.name);
  const [tz, setTz] = useState(workspace.timezone);
  const [lang, setLang] = useState(workspace.memory_language);
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState(false);
  const fe = patch.error ? fieldErrors(patch.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const n = name.trim();
    if (!n || n.length > 200) return setClientErr("Name must be 1 to 200 characters.");
    setClientErr("");
    patch.mutate(
      { name: n, timezone: tz, memory_language: lang },
      { onSuccess: () => setSaved(true) },
    );
  }

  const tzOptions = zones.includes(tz) ? zones : [tz, ...zones];
  const langOptions = (MEMORY_LANGUAGES as readonly string[]).includes(lang)
    ? MEMORY_LANGUAGES
    : [lang, ...MEMORY_LANGUAGES];

  return (
    <form onSubmit={submit} noValidate className="max-w-md space-y-4">
      {!canOwn ? (
        <p className="rounded-md bg-neutral-100 p-3 text-sm text-neutral-700">
          Only workspace owners can change these settings.
        </p>
      ) : null}
      <fieldset disabled={!canOwn || patch.isPending} className="space-y-4">
        <Field id="set-name" label="Name" error={clientErr || fe.name}>
          <input
            id="set-name"
            className={inputClass}
            value={name}
            maxLength={200}
            onChange={(e) => setName(e.target.value)}
          />
        </Field>
        <Field id="set-tz" label="Timezone" error={fe.timezone}>
          <select id="set-tz" className={inputClass} value={tz} onChange={(e) => setTz(e.target.value)}>
            {tzOptions.map((z) => (
              <option key={z} value={z}>
                {z}
              </option>
            ))}
          </select>
        </Field>
        <Field
          id="set-lang"
          label="Memory language"
          hint="Language used for memory search indexing."
          error={fe.memory_language}
        >
          <select
            id="set-lang"
            className={inputClass}
            value={lang}
            onChange={(e) => setLang(e.target.value)}
          >
            {langOptions.map((l) => (
              <option key={l} value={l}>
                {l}
              </option>
            ))}
          </select>
        </Field>
      </fieldset>
      {patch.error && Object.keys(fe).length === 0 ? (
        <p role="alert" className="text-sm text-red-700">
          {errorMessage(patch.error)}
        </p>
      ) : null}
      {saved ? (
        <p role="status" className="text-sm text-green-700">
          Settings saved.
        </p>
      ) : null}
      {canOwn ? (
        <Button type="submit" disabled={patch.isPending}>
          {patch.isPending ? "Saving…" : "Save changes"}
        </Button>
      ) : null}
    </form>
  );
}
