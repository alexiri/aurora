"use client";

import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Fold } from "@/components/ui/fold";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { Section } from "@/components/ui/section";
import { formatCount } from "@/lib/format-date";
import { agentTemplateKeys, useAgentHelperText, useAgentHelpers, useAgentTemplate, useDeleteAgentHelpers, useDetachAgentTemplate, type HelperFile } from "@/lib/api/agent-templates";
import type { Agent } from "@/lib/api/queries";

import { ReviewChangesDialog } from "./review-changes-dialog";

/**
 * (C-1117) The agent's Template section on Settings › Manage, above Export and import: "Made from
 * <template> <version>", the helper files with **View** and **Remove** (C-1143), and, when a newer
 * version is offered, the update sentence with **Review changes** (the dialog), **Skip this
 * version** and **Stop following this template**. It is shown only for agents with a `template` or
 * with helper files. Everything the template brought is shown as inert text (SEC-146).
 */
export function AgentTemplateSection({ agent, agentBase }: { agent: Pick<Agent, "id" | "name" | "handle" | "template">; agentBase: string }) {
  const status = useAgentTemplate(agent.id);
  const helpers = useAgentHelpers(agent.id);
  const qc = useQueryClient();
  const template = status.data?.template ?? agent.template ?? null;
  const helperFiles = helpers.data?.items ?? [];
  // Only for agents with a template or with helper files (C-1117). The agent's own `template`
  // shows the section at once; the helper list decides when the agent has helpers but no template.
  const shown = !!template || helperFiles.length > 0;
  const [viewing, setViewing] = useState(false);
  const [reviewing, setReviewing] = useState(false);
  const [stale, setStale] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [detaching, setDetaching] = useState(false);
  const remove = useDeleteAgentHelpers(agent.id);
  const detach = useDetachAgentTemplate(agent.id);
  // A 409 `template_version_changed` (C-1112): the status is read again and the section says the
  // template moved on while the person was reviewing.
  const onStale = () => {
    void qc.invalidateQueries({ queryKey: agentTemplateKeys.status(agent.id) });
    setReviewing(false);
    setStale(true);
  };

  if (!shown) return null;
  const update = status.data?.update ?? null;
  return (
    <Section id="agent-template" title="Template" card>
      <div className="space-y-3 text-sm text-ink-2" data-testid="agent-template-section">
        {template ? (
          <p data-testid="template-made-from">
            Made from <span className="font-semibold text-ink">{template.name}</span> {template.version}
          </p>
        ) : null}
        {status.data && (status.data.notes ?? []).length > 0 ? (
          <Notice tone="accent" role="status" data-testid="template-notes">
            {status.data.notes.map((n, i) => (
              <p key={i}>{n.detail}</p>
            ))}
          </Notice>
        ) : null}
        {status.data && template && !status.data.available ? (
          <p data-testid="template-unavailable">The template is no longer offered here. {agent.name} keeps working as it is.</p>
        ) : null}
        {update ? (
          <>
            <p data-testid="template-update">
              Version <span className="font-semibold text-ink">{update.version}</span> is available. {agent.name} keeps working as it is until you
              apply it.
            </p>
            <div className="flex flex-wrap gap-2">
              <Button type="button" variant="outline" size="sm" onClick={() => setReviewing(true)}>
                Review changes
              </Button>
            </div>
          </>
        ) : null}
        {helpers.isError && template ? <ErrorNotice error={helpers.error} /> : null}
        {status.isError && template ? <ErrorNotice error={status.error} /> : null}
        {helperFiles.length > 0 || helpers.isSuccess ? (
          <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
            <p data-testid="helper-count">
              {helperFiles.length === 1 ? "1 helper file" : `${formatCount(helperFiles.length)} helper files`} in{" "}
              <span className="font-mono text-xs">.aurora/helpers</span>
            </p>
            <div className="flex flex-wrap gap-2">
              {helperFiles.length > 0 ? (
                <Button type="button" variant="outline" size="sm" onClick={() => setViewing(true)}>
                  View
                </Button>
              ) : null}
              {helperFiles.length > 0 ? (
                <Button type="button" variant="outline" size="sm" onClick={() => setRemoving(true)}>
                  Remove
                </Button>
              ) : null}
            </div>
          </div>
        ) : null}
        {remove.isError ? <ErrorNotice error={remove.error} /> : null}
        {detach.isError ? <ErrorNotice error={detach.error} /> : null}
        {template ? (
          <div className="flex flex-wrap gap-2 border-t border-line pt-3">
            <Button type="button" variant="outline" size="sm" onClick={() => setDetaching(true)}>
              Stop following this template
            </Button>
          </div>
        ) : null}
        {stale ? (
          <Notice tone="pending">
            <p data-testid="template-stale">This template was updated. Check the changes, then apply.</p>
          </Notice>
        ) : null}
      </div>
      {viewing ? <HelperFilesDialog agent={agent} files={helperFiles} onClose={() => setViewing(false)} /> : null}
      {reviewing && update ? (
        <ReviewChangesDialog
          agentId={agent.id}
          agentName={agent.name}
          templateName={template?.name ?? ""}
          offer={update}
          agentBase={agentBase}
          onStale={onStale}
          onClose={() => setReviewing(false)}
        />
      ) : null}
      <ConfirmDialog
        open={removing}
        onOpenChange={(o) => !o && setRemoving(false)}
        title="Remove all helper files?"
        description={`The ${formatCount(helperFiles.length)} files in .aurora/helpers are removed from ${agent.name}. If a template update offers them again, applying it brings them back.`}
        confirmLabel="Remove all"
        danger
        busy={remove.isPending}
        onConfirm={() => {
          remove.mutate(null);
          setRemoving(false);
        }}
      />
      <ConfirmDialog
        open={detaching}
        onOpenChange={(o) => !o && setDetaching(false)}
        title="Stop following this template?"
        description={`${agent.name} keeps its helper files and its skills from libraries. ${template?.name ?? "The template"} is no longer mentioned on ${agent.name}, and its updates are no longer offered.`}
        confirmLabel="Stop following"
        busy={detach.isPending}
        onConfirm={() => {
          detach.mutate();
          setDetaching(false);
        }}
      />
    </Section>
  );
}

/**
 * The helper files of the agent (C-1143): one fold per file — path, size, Executable — whose text is
 * read from `getAgentHelper` when it opens and shown as plain text. Every helper file is UTF-8 text
 * (owner decision 2026-11-24), so there is no binary case. A file can be removed here, with a confirm.
 */
function HelperFilesDialog({ agent, files, onClose }: { agent: { id: string; name: string }; files: HelperFile[]; onClose: () => void }) {
  const remove = useDeleteAgentHelpers(agent.id);
  const [removingOne, setRemovingOne] = useState<HelperFile | null>(null);
  return (
    <Dialog open onOpenChange={(o) => !o && !removingOne && onClose()}>
      <DialogContent
        title="Helper files"
        description={`What ${agent.name} keeps in .aurora/helpers. The files are read-only for the agent.`}
        size="md"
        footer={
          <DialogClose asChild>
            <Button variant="outline">Close</Button>
          </DialogClose>
        }
      >
        <div className="space-y-3" data-testid="helper-files">
          {remove.isError ? <ErrorNotice error={remove.error} /> : null}
          {files.map((f) => (
            <div key={f.path} className="flex flex-wrap items-start justify-between gap-x-4 gap-y-1">
              <Fold summary={helperSummary(f)} className="min-w-0 flex-1" data-testid={`helper-${f.path}`}>
                <HelperText agentId={agent.id} path={f.path} executable={f.executable} />
              </Fold>
              <Button type="button" variant="outline" size="sm" onClick={() => setRemovingOne(f)}>
                Remove
              </Button>
            </div>
          ))}
        </div>
      </DialogContent>
      <ConfirmDialog
        open={removingOne !== null}
        onOpenChange={(o) => !o && setRemovingOne(null)}
        title={`Remove ${removingOne?.path ?? "the helper file"}?`}
        description="The file is removed from .aurora/helpers. If a template update offers it again, applying it brings it back."
        confirmLabel="Remove file"
        danger
        busy={remove.isPending}
        onConfirm={() => {
          if (removingOne) remove.mutate(removingOne.path);
          setRemovingOne(null);
        }}
      />
    </Dialog>
  );
}

function helperSummary(f: HelperFile) {
  return (
    <>
      <span className="font-mono text-xs">{f.path}</span>
      <span className="font-normal text-muted">
        {" "}
        · {formatCount(f.size)} bytes{f.executable ? " · Executable" : ""}
      </span>
    </>
  );
}

/** The text of one helper file, read when the dialog opens. Shown as plain text, never rendered (SEC-146). */
function HelperText({ agentId, path, executable }: { agentId: string; path: string; executable: boolean }) {
  const text = useAgentHelperText(agentId, path);
  if (text.isError) return <ErrorNotice error={text.error} />;
  if (text.isPending) return <p role="status">Reading {path}…</p>;
  return (
    <pre
      tabIndex={0}
      aria-label={`${path}${executable ? " (executable)" : ""} as text`}
      className="max-w-full overflow-x-auto whitespace-pre-wrap rounded-control border border-line bg-canvas p-2 font-mono text-xs text-ink [overflow-wrap:anywhere]"
    >
      {text.data}
    </pre>
  );
}
