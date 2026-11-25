"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";

import { Altitude } from "@/components/ui/altitude";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { DropZone } from "@/components/ui/drop-zone";
import { CheckboxField } from "@/components/ui/form";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { SecretOnce } from "@/components/ui/secret-once";
import { Section } from "@/components/ui/section";
import type { Agent } from "@/lib/api/queries";
import { exportZip, ExportError, parseExport, type AgentExport } from "@/lib/export/agent-export";
import {
  DEFAULT_CHOICES,
  liveImportApi,
  loadAgentExport,
  loadExisting,
  runImport,
  summarize,
  summaryLine,
  type ImportChoices,
  type ImportReport,
} from "@/lib/export/agent-import";
import { MAX_ZIP_BYTES, readZip, ZipError } from "@/lib/export/zip";
import { useVerb } from "@/lib/palette-verb";

import { SaveTemplateDialog } from "./save-template";

type ExportAgent = Pick<Agent, "id" | "name" | "handle" | "brief" | "model_id" | "proactive_messaging" | "org_id" | "workspace_id" | "avatar_url">;

/** Hand the zip to the browser as a download. */
function download(fileName: string, bytes: Uint8Array) {
  const url = URL.createObjectURL(new Blob([bytes as BlobPart], { type: "application/zip" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/**
 * (P5-REFINE-19, `export.agent`, Expert) **Export <Agent>…** and **Import into <Agent>…** on the agent's Settings page, and
 * the palette verbs of the same names. The zip is built and read in the browser; nothing is uploaded as a file.
 */
export function AgentExportImport({ agent }: { agent: ExportAgent }) {
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [saving, setSaving] = useState(false);
  useVerb("export-agent", () => setExporting(true));
  useVerb("import-agent", () => setImporting(true));
  return (
    <Altitude id="export.agent" level="expert">
      <Section
        id="agent-export-import"
        title="Export and import"
        description={`Take ${agent.name} somewhere else, or bring what another agent knows. Secrets, accounts and conversations are not included.`}
      >
        <div className="flex flex-wrap gap-2">
          <Button type="button" variant="outline" size="sm" onClick={() => setExporting(true)}>
            Export {agent.name}…
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={() => setImporting(true)}>
            Import into {agent.name}…
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={() => setSaving(true)}>
            Save as template…
          </Button>
        </div>
      </Section>
      {exporting ? <ExportDialog agent={agent} onClose={() => setExporting(false)} /> : null}
      {importing ? <ImportDialog agent={agent} onClose={() => setImporting(false)} /> : null}
      {saving ? <SaveTemplateDialog agent={agent} onClose={() => setSaving(false)} /> : null}
    </Altitude>
  );
}

function ExportDialog({ agent, onClose }: { agent: ExportAgent; onClose: () => void }) {
  const [state, setState] = useState<{ data: AgentExport; omitted: string[] } | null>(null);
  const [error, setError] = useState<unknown>(null);
  // Read when the dialog opens.
  useEffect(() => {
    let live = true;
    loadAgentExport(agent).then(
      (r) => live && setState(r),
      (e: unknown) => live && setError(e),
    );
    return () => {
      live = false;
    };
  }, [agent]);
  const s = state ? summarize(state.data) : null;
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent
        title={`Export ${agent.name}`}
        description="A zip with its instructions, memories, skills and automations. Secrets, accounts and conversations are not included."
        size="md"
        footer={
          <div className="flex flex-wrap justify-end gap-2">
            <DialogClose asChild>
              <Button variant="outline">Cancel</Button>
            </DialogClose>
            <Button
              disabled={!state}
              onClick={() => {
                if (!state) return;
                const z = exportZip(state.data, agent.handle);
                download(z.fileName, z.bytes);
                onClose();
              }}
            >
              Download zip
            </Button>
          </div>
        }
      >
        <div className="space-y-3 text-sm text-ink-2">
          {error ? <ErrorNotice error={error} /> : null}
          {!state && !error ? <p role="status">Reading {agent.name}…</p> : null}
          {state ? <p data-testid="export-summary">{summaryLine(state.data) || "Nothing to export yet."}</p> : null}
          {s && s.webhooks > 0 ? <p>Webhook addresses and secrets stay behind: an imported webhook gets new ones.</p> : null}
          {state?.data.automations.some((a) => a.kind === "schedule" && "run_at" in a.cadence) ? (
            <p>A one-time schedule keeps its date, time and time zone. An import creates it paused: check the time, then turn it on.</p>
          ) : null}
          {state?.omitted.map((o) => (
            <Notice key={o} tone="neutral" role="status">
              {o}
            </Notice>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function ImportDialog({ agent, onClose }: { agent: ExportAgent; onClose: () => void }) {
  const qc = useQueryClient();
  const [parsed, setParsed] = useState<AgentExport | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [choices, setChoices] = useState<ImportChoices>(DEFAULT_CHOICES);
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [report, setReport] = useState<ImportReport | null>(null);
  const [secrets, setSecrets] = useState<{ name: string; secret: string }[]>([]);
  const [error, setError] = useState<unknown>(null);
  const input = useRef<HTMLInputElement>(null);

  async function pick(file: File | undefined) {
    setProblem(null);
    setParsed(null);
    if (!file) return;
    if (file.size > MAX_ZIP_BYTES) return setProblem("The zip is larger than 5 MB.");
    try {
      const entries = await readZip(new Uint8Array(await file.arrayBuffer()));
      const e = parseExport(entries);
      setParsed(e);
      // An avatar the agent already has is replaced only when the person ticks it.
      setChoices({ ...DEFAULT_CHOICES, instructions: e.instructions.trim() !== "", avatar: e.avatar !== undefined && !agent.avatar_url });
    } catch (e) {
      setProblem(e instanceof ZipError || e instanceof ExportError ? e.message : "This file could not be read as an Aurora agent export.");
    }
  }

  async function start() {
    if (!parsed) return;
    setError(null);
    try {
      const existing = await loadExisting(agent);
      const total = summarize(parsed);
      setProgress({ done: 0, total: total.memories + total.skills + total.schedules + total.webhooks + 1 + (total.avatar ? 1 : 0) });
      const r = await runImport(parsed, choices, existing, liveImportApi(agent.id), (done, all) => setProgress({ done, total: all }));
      setReport(r);
      setSecrets(r.secrets);
      void qc.invalidateQueries();
    } catch (e) {
      setError(e);
    } finally {
      setProgress(null);
    }
  }

  const s = parsed ? summarize(parsed) : null;
  const busy = progress !== null;
  const nothing = !choices.memories && !choices.skills && !choices.schedules && !choices.webhooks && !choices.avatar && !(choices.instructions && choices.instructionsMode === "replace");
  return (
    <>
      <Dialog open onOpenChange={(o) => !o && !busy && onClose()}>
        <DialogContent
          title={`Import into ${agent.name}`}
          description="Choose an Aurora export (.zip). Nothing is uploaded: it is read here, then created through the usual pages."
          size="md"
          footer={
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button variant="outline" disabled={busy}>
                  {report ? "Close" : "Cancel"}
                </Button>
              </DialogClose>
              {report ? null : (
                <Button disabled={!parsed || busy || nothing} onClick={() => void start()}>
                  {busy ? `Importing… ${progress.done} of ${progress.total}` : "Import"}
                </Button>
              )}
            </div>
          }
        >
          <div className="space-y-4">
            {report ? (
              <ImportResult report={report} />
            ) : (
              <>
                <DropZone onFiles={(files) => void pick(files[0])} label="Drop the zip to read it">
                  <div className="space-y-2 rounded-card border border-dashed border-line-strong p-4 text-sm">
                    <input ref={input} type="file" accept=".zip,application/zip" aria-label="Aurora export (.zip)" className="block w-full text-sm" onChange={(e) => void pick(e.target.files?.[0])} />
                    <p className="text-xs text-muted">Up to 5 MB and 200 files.</p>
                  </div>
                </DropZone>
                {problem ? (
                  <Notice tone="danger" role="alert">
                    {problem}
                  </Notice>
                ) : null}
                {parsed && s ? (
                  <fieldset className="space-y-3" disabled={busy}>
                    <legend className="text-sm font-medium text-ink">
                      {parsed.name} · <span data-testid="import-summary">{summaryLine(parsed)}</span>
                    </legend>
                    {s.instructions ? (
                      <div className="space-y-2">
                        <CheckboxField id="imp-instructions" label="Instructions" checked={choices.instructions} onChange={(v) => setChoices({ ...choices, instructions: v })} />
                        {choices.instructions ? (
                          <div className="ml-6 space-y-1 text-sm text-ink-2" role="radiogroup" aria-label="Instructions">
                            <label className="flex items-center gap-2">
                              <input type="radio" name="imp-mode" checked={choices.instructionsMode === "keep"} onChange={() => setChoices({ ...choices, instructionsMode: "keep" })} />
                              Keep mine
                            </label>
                            <label className="flex items-center gap-2">
                              <input type="radio" name="imp-mode" checked={choices.instructionsMode === "replace"} onChange={() => setChoices({ ...choices, instructionsMode: "replace" })} />
                              Replace instructions
                            </label>
                          </div>
                        ) : null}
                      </div>
                    ) : null}
                    {s.avatar ? (
                      <CheckboxField id="imp-avatar" label="Avatar" hint={agent.avatar_url ? "Replaces the avatar this agent has now." : undefined} checked={choices.avatar} onChange={(v) => setChoices({ ...choices, avatar: v })} />
                    ) : null}
                    {s.memories > 0 ? <CheckboxField id="imp-memories" label={`${s.memories} ${s.memories === 1 ? "memory" : "memories"}`} checked={choices.memories} onChange={(v) => setChoices({ ...choices, memories: v })} /> : null}
                    {s.skills > 0 ? <CheckboxField id="imp-skills" label={`${s.skills} ${s.skills === 1 ? "skill" : "skills"}`} checked={choices.skills} onChange={(v) => setChoices({ ...choices, skills: v })} /> : null}
                    {s.schedules > 0 ? (
                      <CheckboxField id="imp-schedules" label={`${s.schedules} ${s.schedules === 1 ? "schedule" : "schedules"}`} hint="Created paused. Check the schedule, then turn it on." checked={choices.schedules} onChange={(v) => setChoices({ ...choices, schedules: v })} />
                    ) : null}
                    {s.webhooks > 0 ? (
                      <CheckboxField id="imp-webhooks" label={`${s.webhooks} ${s.webhooks === 1 ? "webhook" : "webhooks"}`} hint="Created paused, with a new secret shown once." checked={choices.webhooks} onChange={(v) => setChoices({ ...choices, webhooks: v })} />
                    ) : null}
                    <p className="text-xs text-muted">Access, accounts and the AI key are never changed. Items whose name already exists are skipped.</p>
                  </fieldset>
                ) : null}
                {error ? <ErrorNotice error={error} /> : null}
              </>
            )}
          </div>
        </DialogContent>
      </Dialog>
      {/* One secret at a time: each webhook's signing secret, shown once. */}
      <SecretOnce
        secret={secrets[0]?.secret ?? null}
        title={`Copy the signing secret of ${secrets[0]?.name ?? "the webhook"}`}
        description="You won't see this again, so store it somewhere safe now. Its address is on the webhook's card: use Rotate URL to get one."
        onClose={() => setSecrets((all) => all.slice(1))}
      />
    </>
  );
}

function ImportResult({ report }: { report: ImportReport }) {
  return (
    <div className="space-y-3 text-sm" data-testid="import-report" role="status">
      {report.done.length > 0 ? (
        <div>
          <p className="font-medium text-ink">Imported</p>
          <ul className="list-disc pl-5 text-ink-2">
            {report.done.map((d) => (
              <li key={d}>{d}</li>
            ))}
          </ul>
        </div>
      ) : (
        <p className="text-ink-2">Nothing was imported.</p>
      )}
      {report.schedulesPaused > 0 ? <p className="text-ink-2">Check the schedule, then turn it on: imported schedules are paused.</p> : null}
      {report.webhooksPaused > 0 ? <p className="text-ink-2">Imported webhooks are paused until you turn them on.</p> : null}
      {report.skipped.length > 0 ? (
        <ul className="list-disc pl-5 text-ink-2">
          {report.skipped.map((d) => (
            <li key={d}>{d}</li>
          ))}
        </ul>
      ) : null}
      {report.failed.length > 0 ? (
        <Notice tone="danger" role="alert">
          <p className="font-medium">Not imported</p>
          <ul className="list-disc pl-5">
            {report.failed.map((d) => (
              <li key={d}>{d}</li>
            ))}
          </ul>
        </Notice>
      ) : null}
    </div>
  );
}
