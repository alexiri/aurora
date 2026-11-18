"use client";

import { useMemo, useState } from "react";

import { Button } from "@/components/ui/button";
import { Notice } from "@/components/ui/notice";
import { errorMessage } from "@/lib/api/errors";
import { previewWebhook, type WebhookConfig, type WebhookPreview } from "@/lib/api/webhook";
import { configIssues, initialInputs, invalidSlot, valuesFromInputs, type Issue } from "@/lib/webhook/logic";

import { SlotValues } from "./slot-values";

/**
 * Preview (C-913): the server renders the template with sample values and sends nothing (C-909). Secret values come back
 * as `«secret:name»`. A refusal is mapped onto the editor: `webhook_config_invalid` paths to their controls through
 * `onIssues`, `invalid_slot` to the sample value it names.
 */
export function PreviewPanel({
  config,
  secretNames,
  onIssues,
}: {
  config: WebhookConfig;
  secretNames: string[];
  onIssues: (issues: Issue[]) => void;
}) {
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [result, setResult] = useState<WebhookPreview | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputs = useMemo(() => ({ ...initialInputs(config.slots), ...typed }), [config.slots, typed]);

  async function run() {
    const { values, errors: local } = valuesFromInputs(config.slots, inputs);
    setErrors(local);
    setFailure(null);
    onIssues([]);
    if (Object.keys(local).length > 0) return;
    setBusy(true);
    try {
      setResult(await previewWebhook({ ...config, secret_value_names: secretNames }, values));
    } catch (err) {
      setResult(null);
      const issues = configIssues(err);
      const slot = invalidSlot(err);
      if (issues.length > 0) {
        onIssues(issues);
        setFailure("The preview could not be made. Fix what is marked above, then try again.");
      } else if (slot) setErrors({ [slot.slot]: slot.message });
      else setFailure(errorMessage(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-3" data-testid="webhook-preview">
      <p className="text-sm text-ink-2">Sample values for the preview. Nothing is sent.</p>
      <SlotValues slots={config.slots} inputs={inputs} errors={errors} onChange={(n, v) => setTyped((t) => ({ ...t, [n]: v }))} idp="pv" />
      <Button type="button" variant="outline" disabled={busy} onClick={() => void run()}>
        {busy ? "Rendering…" : "Preview"}
      </Button>
      {failure ? (
        <Notice tone="danger" role="alert">
          {failure}
        </Notice>
      ) : null}
      {result ? (
        <div className="space-y-2" role="status">
          <p className="text-sm text-ink-2">
            {result.method} as {result.content_type}
            {result.headers.length > 0 ? `, with the headers ${result.headers.join(", ")}` : ""}.
          </p>
          {/* Text, never HTML: the body is shown inside <pre> as a string. */}
          <pre data-testid="preview-body" className="max-h-64 overflow-auto whitespace-pre-wrap break-all rounded-control border border-line bg-canvas p-3 font-mono text-xs text-ink">
            {result.body}
          </pre>
        </div>
      ) : null}
    </div>
  );
}
