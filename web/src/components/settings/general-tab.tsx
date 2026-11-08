"use client";

import { useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Field, inputClass } from "@/components/ui/field";
import { Form, FormActions } from "@/components/ui/form";
import { Notice } from "@/components/ui/notice";
import { usePatchWorkspace } from "@/lib/api/account";
import { errorMessage, fieldErrors } from "@/lib/api/errors";
import { byoOptionsFrom, useEffectiveGovernance, type ByoPosture } from "@/lib/api/governance";
import type { Workspace } from "@/lib/api/queries";
import { MEMORY_LANGUAGES, timeZones } from "@/lib/nav/landing";

const BYO_LABEL: Record<ByoPosture, string> = {
  enabled: "Enabled: members can create BYO agents",
  owners_only: "Owners only",
  disabled: "Disabled: no BYO agents",
};

export function GeneralTab({ workspace, canOwn }: { workspace: Workspace; canOwn: boolean }) {
  const patch = usePatchWorkspace(workspace.id);
  const zones = timeZones();
  const [name, setName] = useState(workspace.name);
  const [tz, setTz] = useState(workspace.timezone);
  const [lang, setLang] = useState(workspace.memory_language);
  // (RFE-025) "" = inherit; options never go below the instance/org posture.
  const [byoPosture, setByoPosture] = useState<string>(workspace.byo_posture ?? "");
  const orgGov = useEffectiveGovernance(workspace.org_id);
  const inherited = (orgGov.data?.byo.posture.value ?? "enabled") as ByoPosture;
  const [clientErr, setClientErr] = useState("");
  const [saved, setSaved] = useState(false);
  const fe = patch.error ? fieldErrors(patch.error) : {};

  function submit(e: FormEvent) {
    e.preventDefault();
    setSaved(false);
    const n = name.trim();
    if (!n || n.length > 200) return setClientErr("Name must be 1 to 200 characters.");
    setClientErr("");
    const body: Parameters<typeof patch.mutate>[0] = { name: n, timezone: tz, memory_language: lang };
    if (byoPosture !== (workspace.byo_posture ?? "")) body.byo_posture = byoPosture || null;
    patch.mutate(body, { onSuccess: () => setSaved(true) });
  }

  const tzOptions = zones.includes(tz) ? zones : [tz, ...zones];
  const langOptions = (MEMORY_LANGUAGES as readonly string[]).includes(lang)
    ? MEMORY_LANGUAGES
    : [lang, ...MEMORY_LANGUAGES];

  return (
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
        <Field
          id="set-byo"
          label="BYO agent posture"
          hint={`Who may create BYO agents here. A workspace can only tighten what the organization or instance sets (currently ${inherited}).`}
          error={fe.byo_posture}
        >
          <select id="set-byo" className={inputClass} value={byoPosture} onChange={(e) => setByoPosture(e.target.value)}>
            <option value="">Inherit ({inherited})</option>
            {byoOptionsFrom(inherited).map((p) => (
              <option key={p} value={p}>
                {BYO_LABEL[p]}
              </option>
            ))}
          </select>
        </Field>
        {workspace.effective_byo_posture ? (
          <p className="text-sm text-ink-2">
            Effective BYO posture:{" "}
            <Badge tone={workspace.effective_byo_posture.value === "disabled" ? "danger" : "accent"}>
              {workspace.effective_byo_posture.value}
            </Badge>{" "}
            <span className="text-muted">set by {workspace.effective_byo_posture.level}</span>
          </p>
        ) : null}
      </fieldset>
      {patch.error && Object.keys(fe).length === 0 ? (
        <Notice tone="danger" role="alert">
          {errorMessage(patch.error)}
        </Notice>
      ) : null}
      {saved ? (
        <Notice tone="accent" role="status">
          Settings saved.
        </Notice>
      ) : null}
      {canOwn ? (
        <FormActions>
          <Button type="submit" size="lg" disabled={patch.isPending}>
            {patch.isPending ? "Saving…" : "Save changes"}
          </Button>
        </FormActions>
      ) : null}
    </Form>
  );
}
