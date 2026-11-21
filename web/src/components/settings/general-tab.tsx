"use client";

import { useState, type FormEvent } from "react";

import { Altitude, AltitudeScope, MoreControlsLine } from "@/components/ui/altitude";
import { Field, inputClass } from "@/components/ui/field";
import { Form } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { SaveBar, useSaveBar } from "@/components/ui/save-bar";
import { Select } from "@/components/ui/select";
import { usePatchWorkspace } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import type { Workspace } from "@/lib/api/queries";
import { MEMORY_LANGUAGES, memoryLanguageName, timeZones } from "@/lib/nav/landing";

export function GeneralTab({ workspace, canOwn }: { workspace: Workspace; canOwn: boolean }) {
  const patch = usePatchWorkspace(workspace.id);
  const zones = timeZones();
  const [name, setName] = useState(workspace.name);
  const [tz, setTz] = useState(workspace.timezone);
  const [lang, setLang] = useState(workspace.memory_language);
  const [clientErr, setClientErr] = useState("");
  const bar = useSaveBar();
  const [savedName, setSavedName] = useState<string | null>(null);
  // What is saved: the form's start, and again after each save (the workspace-7 finding: "Saved" appears).
  const [base, setBase] = useState({ name: workspace.name, tz: workspace.timezone, lang: workspace.memory_language });
  // The page no longer remounts this form when the workspace changes (a remount after the save dropped "Saved. The
  // workspace is now called …"): a workspace changed from elsewhere starts the form again from what is stored.
  const [seenAt, setSeenAt] = useState(workspace.updated_at);
  if (workspace.updated_at !== seenAt) {
    setSeenAt(workspace.updated_at);
    setName(workspace.name);
    setTz(workspace.timezone);
    setLang(workspace.memory_language);
    setBase({ name: workspace.name, tz: workspace.timezone, lang: workspace.memory_language });
  }
  const dirty = name !== base.name || tz !== base.tz || lang !== base.lang;
  const fe = patch.error ? fieldErrors(patch.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    bar.clear();
    const n = name.trim();
    if (!n || n.length > 200) return setClientErr("Name must be 1 to 200 characters.");
    setClientErr("");
    patch.mutate(
      { name: n, timezone: tz, memory_language: lang },
      {
        onSuccess: () => {
          setSavedName(n !== base.name ? n : null);
          setBase({ name, tz, lang });
          bar.flash();
        },
      },
    );
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
            label="Language agents remember in"
            hint="Language used for memory search indexing."
            error={fe.memory_language}
          >
            <Select id="set-lang" value={lang} onChange={setLang} options={langOptions.map((l) => ({ value: l, label: memoryLanguageName(l) }))} />
          </Field>
        </Altitude>
      </fieldset>
      {bar.saved && savedName ? (
        <p role="status" className="text-sm text-accent-ink" data-testid="saved-name">
          Saved. The workspace is now called {savedName}.
        </p>
      ) : null}
      {canOwn ? (
        <SaveBar
          dirty={dirty}
          saving={patch.isPending}
          saved={bar.saved}
          errorCount={Object.keys(fe).length + (clientErr ? 1 : 0)}
          error={patch.error && Object.keys(fe).length === 0 ? errorMessage(patch.error) : undefined}
          onDiscard={() => {
            setName(base.name);
            setTz(base.tz);
            setLang(base.lang);
            setClientErr("");
            patch.reset();
          }}
        />
      ) : null}
      <MoreControlsLine />
    </Form>
    </AltitudeScope>
  );
}
