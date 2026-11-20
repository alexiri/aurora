"use client";

import { useState, type FormEvent } from "react";

import { Altitude, AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { ErrorNotice, Notice, SavedNotice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { usePatchWorkspace } from "@/lib/api/account";
import { fieldErrors } from "@/lib/api/errors";
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
    patch.mutate({ name: n, timezone: tz, memory_language: lang }, { onSuccess: () => setSaved(true) });
  }

  const tzOptions = zones.includes(tz) ? zones : [tz, ...zones];
  const langOptions = (MEMORY_LANGUAGES as readonly string[]).includes(lang)
    ? MEMORY_LANGUAGES
    : [lang, ...MEMORY_LANGUAGES];

  return (
    <AltitudeScope>
    <Form onSubmit={submit} noValidate className="max-w-md">
      {!canOwn ? <Notice role="status">Only workspace owners can change these settings.</Notice> : null}
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
        <Field id="set-tz" label="Time zone" error={fe.timezone}>
          <Select id="set-tz" value={tz} onChange={setTz} options={tzOptions.map((z) => ({ value: z, label: z }))} />
        </Field>
        <Altitude id="ws.memlang" level="expert" promote={!!fe.memory_language}>
          <Field
            id="set-lang"
            label="Memory language"
            hint="Language used for memory search indexing."
            error={fe.memory_language}
          >
            <Select id="set-lang" value={lang} onChange={setLang} options={langOptions.map((l) => ({ value: l, label: l }))} />
          </Field>
        </Altitude>
      </fieldset>
      {patch.error && Object.keys(fe).length === 0 ? (
        <ErrorNotice error={patch.error} />
      ) : null}
      <SavedNotice show={saved}>Settings saved.</SavedNotice>
      {canOwn ? (
        <FormActions>
          <Button type="submit" size="lg" disabled={patch.isPending}>
            {patch.isPending ? "Saving…" : "Save changes"}
          </Button>
        </FormActions>
      ) : null}
      <MoreControlsLine />
    </Form>
    </AltitudeScope>
  );
}
