"use client";

import { useState } from "react";

import { Button } from "@/components/ui/button";
import { CodeField } from "@/components/ui/code-field";
import { Field, inputClass } from "@/components/ui/field";
import { Notice } from "@/components/ui/notice";
import { Select } from "@/components/ui/select";
import { useWebhookPresets, type ContentType, type WebhookConfig } from "@/lib/api/webhook";
import { configFromPreset, CONTENT_TYPES, firstMessage, LIMITS, suggestPreset, unmatchedIssues, type Issue } from "@/lib/webhook/logic";

import { ExamplePicker } from "./example-picker";
import { PreviewPanel } from "./preview-panel";
import { SlotTable } from "./slot-table";

/**
 * Step 2, "What the receiver expects" (C-913): a preset (suggested from the address, never forced), an example to start
 * from, and the raw template and slot editor, then Preview. The configuration lives in the caller; this edits it.
 */
export function ReceiverStep({
  url,
  config,
  onConfig,
  secretNames,
  onPresetSecrets,
  issues,
  onIssues,
}: {
  /** The address typed in step 1, for the suggestion; absent when editing (the address is never shown again). */
  url?: string;
  config: WebhookConfig;
  onConfig: (c: WebhookConfig) => void;
  secretNames: string[];
  /** A preset names the secret values the person must enter; the caller keeps the values already typed. */
  onPresetSecrets: (names: string[]) => void;
  issues: Issue[];
  onIssues: (issues: Issue[]) => void;
}) {
  const presets = useWebhookPresets();
  const [docKey, setDocKey] = useState(0);
  const list = presets.data ?? [];
  const suggested = url ? suggestPreset(list, url) : null;
  const apply = (id: string) => {
    const p = list.find((x) => x.id === id);
    if (!p) return onConfig({ ...config, preset: null });
    onConfig(configFromPreset(p, config));
    onPresetSecrets(p.secret_value_names);
    setDocKey((k) => k + 1);
    onIssues([]);
  };
  const general = unmatchedIssues(issues);

  return (
    <div className="space-y-6">
      {general.length > 0 ? (
        <Notice tone="danger" role="alert">
          <ul className="list-disc pl-4">
            {general.map((i, n) => (
              <li key={n}>{i.message}</li>
            ))}
          </ul>
        </Notice>
      ) : null}

      <section className="space-y-3" aria-labelledby="wh-preset-h">
        <h3 id="wh-preset-h" className="font-display text-base font-semibold text-ink">
          Start from a preset
        </h3>
        {suggested && config.preset !== suggested.id ? (
          <Notice tone="accent" role="status" className="flex flex-wrap items-center gap-2">
            <span>This address looks like {suggested.name}.</span>
            <Button type="button" size="sm" variant="outline" onClick={() => apply(suggested.id)}>
              Use the {suggested.name} preset
            </Button>
          </Notice>
        ) : null}
        {presets.isError ? <Notice role="status">The presets could not be loaded. You can still write the template yourself.</Notice> : null}
        <Field id="wh-preset" label="Preset (optional)" hint="A preset fills in the template, the values and the content type. You can change everything afterwards.">
          <select id="wh-preset" className={inputClass} value={config.preset ?? ""} disabled={presets.isPending} onChange={(e) => apply(e.target.value)}>
            <option value="">None: write it myself</option>
            {list.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
                {suggested?.id === p.id ? " (suggested)" : ""}
              </option>
            ))}
          </select>
        </Field>
      </section>

      <details className="rounded-control border border-line p-3">
        <summary className="min-h-9 cursor-pointer text-sm font-semibold text-ink">Start from an example</summary>
        <div className="mt-3">
          <ExamplePicker
            onApply={({ template, slots }) => {
              onConfig({ ...config, content_type: "application/json", template, slots, preset: null });
              setDocKey((k) => k + 1);
              onIssues([]);
            }}
          />
        </div>
      </details>

      <section className="space-y-3" aria-labelledby="wh-template-h">
        <h3 id="wh-template-h" className="font-display text-base font-semibold text-ink">
          The template
        </h3>
        <Field id="wh-ct" label="Content type" error={firstMessage(issues, "content_type")}>
          <Select id="wh-ct" value={config.content_type} onChange={(v) => onConfig({ ...config, content_type: v as ContentType })} options={CONTENT_TYPES.map((c) => ({ value: c.value, label: c.label }))} />
        </Field>
        <CodeField
          label="Template"
          docKey={docKey}
          initial={config.template}
          fileName={config.content_type === "application/json" ? "template.json" : "template.txt"}
          onChange={(t) => onConfig({ ...config, template: t })}
          error={firstMessage(issues, "template")}
          hint={`Write {{name}} where the agent fills in a value and {{secret:name}} for a secret value. ${config.template.length.toLocaleString()} of ${LIMITS.template.toLocaleString()} characters.`}
        />
        <h4 className="text-sm font-semibold text-ink">Values the agent fills in</h4>
        <SlotTable slots={config.slots} contentType={config.content_type} issues={issues} onChange={(slots) => onConfig({ ...config, slots })} />
      </section>

      <section className="space-y-3" aria-labelledby="wh-preview-h">
        <h3 id="wh-preview-h" className="font-display text-base font-semibold text-ink">
          Preview
        </h3>
        <PreviewPanel config={config} secretNames={secretNames} onIssues={onIssues} />
      </section>
    </div>
  );
}
