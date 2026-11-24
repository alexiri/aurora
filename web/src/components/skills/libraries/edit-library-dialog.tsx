"use client";

import { useId, useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/confirm-dialog";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { ErrorNotice } from "@/components/ui/notice";
import { SecretInput } from "@/components/ui/secret-input";
import { usePatchSkillLibrary, type LibraryOwner, type PatchSkillLibraryRequest, type SkillLibrary } from "@/lib/api/skill-libraries";
import { RepositoryAddressFields } from "@/components/ui/repository-address";
import { formatDay } from "@/lib/format-date";

import { addressError } from "./words";

/**
 * Edit a library (D-450(d), C-1129): label, branch or tag, folder and the access token (Replace, Remove, Add a token).
 * The address is fixed: another repository is a new library; only a library migrated without one takes **Set address**.
 * Saving re-reads the repository on the server; a failure keeps the dialog open with the reason.
 */
export function EditLibraryDialog({ library, orgId, open, onOpenChange }: { library: SkillLibrary | null; orgId: LibraryOwner; open: boolean; onOpenChange: (open: boolean) => void }) {
  return open && library ? <EditFlow key={library.id} library={library} orgId={orgId} onOpenChange={onOpenChange} /> : null;
}

function EditFlow({ library: l, orgId, onOpenChange }: { library: SkillLibrary; orgId: LibraryOwner; onOpenChange: (open: boolean) => void }) {
  const formId = useId();
  const patch = usePatchSkillLibrary(orgId);
  const [label, setLabel] = useState(l.label);
  const [ref, setRef] = useState(l.ref ?? "");
  const [path, setPath] = useState(l.path ?? "");
  const [url, setUrl] = useState("");
  const [replacing, setReplacing] = useState(false);
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("");
  const [removing, setRemoving] = useState(false);
  const noAddress = l.url === null;

  function save(e: FormEvent) {
    e.preventDefault();
    const body: PatchSkillLibraryRequest = {};
    if (label.trim() !== l.label) body.label = label.trim();
    if (ref.trim() !== (l.ref ?? "")) body.ref = ref.trim();
    if (path.trim() !== (l.path ?? "")) body.path = path.trim();
    if (noAddress && url.trim()) body.url = url.trim();
    if (replacing && token.trim()) {
      body.token = token.trim();
      if (username.trim()) body.token_username = username.trim();
    }
    if (Object.keys(body).length === 0) return onOpenChange(false);
    patch.mutate({ id: l.id, body }, { onSuccess: () => onOpenChange(false) });
  }

  const urlProblem = addressError(patch.error);
  return (
    <>
      <Dialog open onOpenChange={onOpenChange}>
        <DialogContent
          title={`Edit ${l.label}`}
          description="Saving reads the repository again with the new settings. Skills already added keep the content they were pinned to."
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
            {patch.error && !urlProblem ? <ErrorNotice error={patch.error} /> : null}
            <Field id="edit-label" label="Label">
              <input id="edit-label" className={inputClass} value={label} maxLength={80} autoComplete="off" onChange={(e) => setLabel(e.target.value)} />
            </Field>
            {noAddress ? (
              <Field id="edit-url" label="Set address" error={urlProblem ?? undefined} hint="This library has no git address yet. Enter the https address of its repository, or its page.">
                <input id="edit-url" className={inputClass} value={url} inputMode="url" autoComplete="off" spellCheck={false} onChange={(e) => setUrl(e.target.value)} />
              </Field>
            ) : (
              <div className="space-y-1.5">
                <p className="text-sm font-medium text-ink">Repository address</p>
                <p className="break-all font-mono text-xs text-ink" data-testid="library-address">
                  {l.url}
                </p>
                <p className="text-xs text-muted">To use another repository, add a new library.</p>
              </div>
            )}
            <RepositoryAddressFields idPrefix="edit" branch={ref} onBranch={setRef} path={path} onPath={setPath} pathHint="The folder that holds the skill folders. Default: the repository root" />
            <div className="space-y-2">
              <p className="text-sm font-medium text-ink">Access token</p>
              {l.token_set ? (
                <p className="text-sm text-ink-2">A token is saved{l.token_updated_at ? ` (since ${formatDay(l.token_updated_at)})` : ""}.</p>
              ) : (
                <p className="text-sm text-ink-2">No token.</p>
              )}
              {replacing ? (
                <div className="space-y-3">
                  <Field id="edit-token" label="New access token" hint="Use a token that can only read this repository. Aurora only reads.">
                    <SecretInput id="edit-token" value={token} onChange={setToken} aria-label="New access token" />
                  </Field>
                  <Field id="edit-user" label="User name" hint="Most hosts accept any name. Bitbucket: x-token-auth">
                    <input id="edit-user" className={inputClass} value={username} autoComplete="off" spellCheck={false} onChange={(e) => setUsername(e.target.value)} />
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
                    {l.token_set ? "Replace" : "Add a token"}
                  </Button>
                  {l.token_set ? (
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
        description="Aurora can no longer read this repository if it is private. The library's skills keep their content."
        confirmLabel="Remove token"
        danger
        busy={patch.isPending}
        onConfirm={() => patch.mutate({ id: l.id, body: { token: null } }, { onSuccess: () => setRemoving(false), onError: () => setRemoving(false) })}
      />
    </>
  );
}
