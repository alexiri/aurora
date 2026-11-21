"use client";

import { useId, useState, type FormEvent } from "react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { Field, inputClass } from "@/components/ui/field";
import { Fold } from "@/components/ui/fold";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { SecretInput } from "@/components/ui/secret-input";
import { errorMessage } from "@/lib/api/errors";
import {
  shortSha,
  useCheckLibrary,
  useCreateSkillLibrary,
  type CheckLibraryRequest,
  type LibraryCheck,
  type LibraryOwner,
} from "@/lib/api/skill-libraries";

import { addableWords, addressError, isRateLimited, RATE_LIMITED_SENTENCE, resolvedWords } from "./words";

/**
 * Add a library (D-450(c), C-1129): *Where is it?* → **Check** (reads the repository, stores nothing) → *What Aurora
 * found* → **Add library**. One dialog for both levels: `orgId` is the organization, or `null` for the instance's own.
 * Everything Aurora reports from the repository is shown as plain text.
 */
export function AddLibraryDialog({ orgId, open, onOpenChange }: { orgId: LibraryOwner; open: boolean; onOpenChange: (open: boolean) => void }) {
  // The flow is mounted only while open, so every visit starts empty.
  return open ? <AddLibraryFlow orgId={orgId} onOpenChange={onOpenChange} /> : null;
}

function AddLibraryFlow({ orgId, onOpenChange }: { orgId: LibraryOwner; onOpenChange: (open: boolean) => void }) {
  const formId = useId();
  const check = useCheckLibrary(orgId);
  const create = useCreateSkillLibrary(orgId);
  const [url, setUrl] = useState("");
  const [ref, setRef] = useState("");
  const [path, setPath] = useState("");
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("");
  const [label, setLabel] = useState("");
  const [found, setFound] = useState<LibraryCheck | null>(null);
  const [missing, setMissing] = useState("");

  const bodyOf = (): CheckLibraryRequest => ({
    url: url.trim(),
    ...(ref.trim() ? { ref: ref.trim() } : {}),
    ...(path.trim() ? { path: path.trim() } : {}),
    ...(token.trim() ? { token: token.trim() } : {}),
    // A user name means nothing without a token (the API refuses it): only sent together.
    ...(token.trim() && username.trim() ? { token_username: username.trim() } : {}),
  });

  function runCheck(e: FormEvent) {
    e.preventDefault();
    if (!url.trim()) return setMissing("Enter the repository's address.");
    setMissing("");
    check.mutate(bodyOf(), {
      onSuccess: (c) => {
        setFound(c);
        setLabel(c.suggested_label);
        create.reset();
      },
    });
  }

  function add(e: FormEvent) {
    e.preventDefault();
    if (!found) return;
    if (!label.trim()) return setMissing("Give the library a label.");
    setMissing("");
    const { token: t, token_username: u } = bodyOf();
    // What was found is what is added: the normalized address with the branch and folder the check resolved.
    create.mutate(
      { label: label.trim(), url: found.url, ...(found.ref ? { ref: found.ref } : {}), ...(found.path ? { path: found.path } : {}), ...(t ? { token: t } : {}), ...(u ? { token_username: u } : {}) },
      { onSuccess: () => onOpenChange(false) },
    );
  }

  const urlProblem = addressError(check.error) ?? addressError(create.error);
  const failed = check.error ?? create.error;
  const showFailure = failed && !urlProblem;
  const busy = check.isPending || create.isPending;

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent
        title="Add a library"
        description={found ? "What Aurora found. Nothing is saved until you add the library." : "Aurora reads the repository to see what it holds. It only ever reads."}
        size="xl"
        footer={
          <div className="flex flex-wrap justify-end gap-2">
            <DialogClose asChild>
              <Button type="button" variant="outline">
                Cancel
              </Button>
            </DialogClose>
            {found ? (
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  setFound(null);
                  create.reset();
                  check.reset();
                }}
              >
                Back
              </Button>
            ) : null}
            <Button type="submit" form={formId} disabled={busy}>
              {found ? "Add library" : "Check"}
            </Button>
          </div>
        }
      >
        <form id={formId} onSubmit={found ? add : runCheck} className="space-y-4" noValidate>
          {showFailure ? (
            isRateLimited(failed) ? (
              <Notice tone="danger" role="alert">
                {RATE_LIMITED_SENTENCE}
              </Notice>
            ) : (
              <ErrorNotice error={failed} format={errorMessage} />
            )
          ) : null}
          {found ? (
            <Found check={found} label={label} onLabel={setLabel} refGiven={!!ref.trim()} pathGiven={!!path.trim()} labelError={missing} />
          ) : (
            <>
              <Field
                id="lib-url"
                label="Repository address"
                error={missing || urlProblem || undefined}
                hint="The address you clone, or the repository's page. For example https://gitlab.example.org/team/skills or https://github.com/acme/skills/tree/main/skills"
              >
                <input id="lib-url" className={inputClass} value={url} inputMode="url" autoComplete="off" spellCheck={false} onChange={(e) => setUrl(e.target.value)} />
              </Field>
              <Field id="lib-ref" label="Branch or tag" hint="Default: the repository's default branch">
                <input id="lib-ref" className={inputClass} value={ref} autoComplete="off" spellCheck={false} onChange={(e) => setRef(e.target.value)} />
              </Field>
              <Field id="lib-path" label="Folder" hint="The folder that holds the skill folders. Default: the repository root">
                <input id="lib-path" className={inputClass} value={path} autoComplete="off" spellCheck={false} onChange={(e) => setPath(e.target.value)} />
              </Field>
              <Fold summary="This repository is private" defaultOpen={!!token}>
                <div className="mt-3 space-y-4">
                  <Field id="lib-token" label="Access token">
                    <SecretInput id="lib-token" value={token} onChange={setToken} aria-label="Access token" />
                  </Field>
                  <Field id="lib-user" label="User name" hint="Most hosts accept any name. Bitbucket: x-token-auth">
                    <input id="lib-user" className={inputClass} value={username} autoComplete="off" spellCheck={false} onChange={(e) => setUsername(e.target.value)} />
                  </Field>
                  <p className="text-xs text-muted">Use a token that can only read this repository. Aurora only reads.</p>
                </div>
              </Fold>
            </>
          )}
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Step 2: what the check found (repository text as plain text) and the label. */
function Found({ check: c, label, onLabel, refGiven, pathGiven, labelError }: { check: LibraryCheck; label: string; onLabel: (v: string) => void; refGiven: boolean; pathGiven: boolean; labelError: string }) {
  const cannot = c.skills.found - c.skills.valid;
  const fromAddress = (!refGiven && !!c.ref) || (!pathGiven && !!c.path);
  const more = c.skills.valid - c.skills.names.length;
  return (
    <>
      {/* The dialog's own width decides the layout (container query), not the viewport. */}
      <div className="@container">
        <dl className="space-y-2 text-sm" data-testid="library-found">
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Repository</dt>
            <dd className="min-w-0 font-mono text-xs text-ink [overflow-wrap:anywhere]">{c.repo}</dd>
          </div>
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Branch</dt>
            <dd className="min-w-0 text-ink [overflow-wrap:anywhere]">{resolvedWords(c)}</dd>
          </div>
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Latest commit</dt>
            <dd className="font-mono text-xs text-ink">{shortSha(c.commit_sha)}</dd>
          </div>
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Folder</dt>
            <dd className="min-w-0 text-ink [overflow-wrap:anywhere]">{c.path ? <span className="font-mono text-xs">{c.path}</span> : "Repository root"}</dd>
          </div>
        </dl>
      </div>
      {fromAddress ? <p className="text-xs text-muted">Branch and folder were taken from the address.</p> : null}
      <div className="space-y-1">
        <p className="text-sm font-medium text-ink">{addableWords(c.skills.valid)}</p>
        {cannot > 0 ? <p className="text-sm text-ink-2">{cannot} can&apos;t be added (open it after adding to see why)</p> : null}
        {c.skills.names.length > 0 ? (
          <ul aria-label="Skills found" className="flex flex-wrap gap-x-3 gap-y-1 font-mono text-xs text-ink-2">
            {c.skills.names.map((n) => (
              <li key={n} className="[overflow-wrap:anywhere]">
                {n}
              </li>
            ))}
            {more > 0 ? <li className="font-sans text-muted">and {more} more</li> : null}
          </ul>
        ) : null}
      </div>
      {c.truncated ? <Notice role="status">This repository has more skill folders than Aurora lists, so only the first were read. Choose a narrower folder to see the rest.</Notice> : null}
      <Field id="lib-label" label="Label" error={labelError || undefined} hint="How this library is named in lists. Up to 80 characters.">
        <input id="lib-label" className={inputClass} value={label} maxLength={80} autoComplete="off" onChange={(e) => onLabel(e.target.value)} />
      </Field>
    </>
  );
}
