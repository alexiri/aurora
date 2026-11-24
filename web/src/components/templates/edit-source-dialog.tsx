"use client";

import { useId, useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { ErrorNotice } from "@/components/ui/notice";
import { Field, inputClass } from "@/components/ui/field";
import { RepositoryAddressFields } from "@/components/ui/repository-address";
import { SecretInput } from "@/components/ui/secret-input";
import { INTERVALS, INTERVAL_LABELS, type Interval } from "@/lib/api/addon-sources";
import { useUpdateTemplateSource, type AgentTemplateSource, type UpdateTemplateSourceRequest } from "@/lib/api/agent-templates";
import { Select } from "@/components/ui/select";
import { formatDay } from "@/lib/format-date";

/**
 * Edit a template repository (C-1114): the schedule, the branch or tag, the folder and the access token (Replace,
 * Remove, Add a token). The address is fixed: another repository is a new source. Saving checks the repository on the
 * server; a failure keeps the dialog open with the reason and nothing changes.
 */
export function EditSourceDialog({ source, open, onOpenChange }: { source: AgentTemplateSource | null; open: boolean; onOpenChange: (open: boolean) => void }) {
  return open && source ? <EditFlow key={source.id} source={source} onOpenChange={onOpenChange} /> : null;
}

function EditFlow({ source: s, onOpenChange }: { source: AgentTemplateSource; onOpenChange: (open: boolean) => void }) {
  const formId = useId();
  const patch = useUpdateTemplateSource();
  const [interval, setInterval] = useState<Interval>(s.update_interval as Interval);
  const [ref, setRef] = useState(s.ref ?? "");
  const [path, setPath] = useState(s.path ?? "");
  const [replacing, setReplacing] = useState(false);
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("");
  const [removing, setRemoving] = useState(false);

  function save(e: FormEvent) {
    e.preventDefault();
    const body: UpdateTemplateSourceRequest = {};
    if (interval !== (s.update_interval as Interval)) body.update_interval = interval;
    if (ref.trim() !== (s.ref ?? "")) body.ref = ref.trim(); // `""` clears: the repository's default branch again
    if (path.trim() !== (s.path ?? "")) body.path = path.trim();
    if (replacing && token.trim()) {
      body.token = token.trim();
      if (username.trim()) body.token_username = username.trim();
    }
    if (Object.keys(body).length === 0) return onOpenChange(false);
    patch.mutate({ id: s.id, body }, { onSuccess: () => onOpenChange(false) });
  }

  return (
    <>
      <Dialog open onOpenChange={onOpenChange}>
        <DialogContent
          title={`Edit ${s.name}`}
          description="Saving reads the repository again with the new settings. Agents made from these templates keep working."
          size="xl"
          footer={
            <div className="flex flex-wrap justify-end gap-2">
              <DialogClose asChild>
                <Button type="button" variant="outline">
                  Cancel
                </Button>
              </DialogClose>
              <Button type="submit" form={formId} disabled={patch.isPending}>
                Save
              </Button>
            </div>
          }
        >
          <form id={formId} onSubmit={save} className="space-y-4" noValidate>
            {patch.error ? <ErrorNotice error={patch.error} /> : null}
            <div className="space-y-1.5">
              <p className="text-sm font-medium text-ink">Repository address</p>
              <p className="break-all font-mono text-xs text-ink" data-testid="template-source-address">
                {s.url}
              </p>
              <p className="text-xs text-muted">To use another repository, add a new one.</p>
            </div>
            <Field id="tpl-schedule" label="Check for updates" hint="How often Aurora reads the repository for new versions.">
              <Select
                id="tpl-schedule"
                value={interval}
                onChange={(v) => setInterval(v as Interval)}
                options={INTERVALS.map((i) => ({ value: i, label: INTERVAL_LABELS[i] }))}
              />
            </Field>
            <RepositoryAddressFields idPrefix="tpl-edit" ref={ref} onRef={setRef} path={path} onPath={setPath} />
            <div className="space-y-2">
              <p className="text-sm font-medium text-ink">Access token</p>
              {s.token_set ? (
                <p className="text-sm text-ink-2">A token is saved{s.token_updated_at ? ` (since ${formatDay(s.token_updated_at)})` : ""}.</p>
              ) : (
                <p className="text-sm text-ink-2">No token.</p>
              )}
              {replacing ? (
                <div className="space-y-3">
                  <Field id="tpl-edit-token" label="New access token" hint="Use a token that can only read this repository. Aurora only reads.">
                    <SecretInput id="tpl-edit-token" value={token} onChange={setToken} aria-label="New access token" />
                  </Field>
                  <Field id="tpl-edit-user" label="User name" hint="Most hosts accept any name. Bitbucket: x-token-auth">
                    <input id="tpl-edit-user" className={inputClass} value={username} autoComplete="off" spellCheck={false} onChange={(e) => setUsername(e.target.value)} />
                  </Field>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => {
                      setReplacing(false);
                      setToken("");
                      setUsername("");
                    }}
                  >
                    Keep the current token
                  </Button>
                </div>
              ) : (
                <div className="flex flex-wrap gap-2">
                  <Button type="button" variant="outline" size="sm" onClick={() => setReplacing(true)}>
                    {s.token_set ? "Replace" : "Add a token"}
                  </Button>
                  {s.token_set ? (
                    <Button type="button" variant="outline" size="sm" onClick={() => setRemoving(true)}>
                      Remove
                    </Button>
                  ) : null}
                </div>
              )}
            </div>
          </form>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={removing}
        onOpenChange={setRemoving}
        title="Remove the access token?"
        description="Aurora can no longer read this repository if it is private. Agents made from these templates keep working."
        confirmLabel="Remove token"
        danger
        busy={patch.isPending}
        onConfirm={() => patch.mutate({ id: s.id, body: { token: null } }, { onSuccess: () => setRemoving(false), onError: () => setRemoving(false) })}
      />
    </>
  );
}
