"use client";

import { useId, useState, type FormEvent } from "react";

import { CHECK_RATE_LIMITED_SENTENCE, isCheckRateLimited, RepositoryAddressFields, repositoryAddressError, resolvedRefWords } from "@/components/ui/repository-address";
import { Button } from "@/components/ui/button";
import { Dialog, DialogClose, DialogContent } from "@/components/ui/dialog";
import { ErrorNotice, Notice } from "@/components/ui/notice";
import { ApiError } from "@/lib/api/client";
import {
  useCreateTemplateSource,
  usePreviewTemplateSource,
  type TemplateSourcePreview,
} from "@/lib/api/agent-templates";
import { errorMessage } from "@/lib/api/errors";
import { shortSha } from "@/lib/api/skill-libraries";

import { foundStateWords, problemWords } from "./words";

/**
 * Add a template repository (C-1114): *the address* → **Check** (reads the repository, stores nothing) →
 * *what Aurora found* → **Add repository**. The skill library's two-step dialog (C-1129) with the template profile;
 * `RepositoryAddressFields` is the one address form. Everything Aurora reports from the repository is plain text.
 */
export function AddSourceDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  // Mounted only while open, so every visit starts empty and **Back** keeps the fields (the flow stays mounted).
  return open ? <AddSourceFlow onOpenChange={onOpenChange} /> : null;
}

function AddSourceFlow({ onOpenChange }: { onOpenChange: (open: boolean) => void }) {
  const formId = useId();
  const check = usePreviewTemplateSource();
  const create = useCreateTemplateSource();
  const [url, setUrl] = useState("");
  const [ref, setRef] = useState("");
  const [path, setPath] = useState("");
  const [token, setToken] = useState("");
  const [username, setUsername] = useState("");
  const [found, setFound] = useState<TemplateSourcePreview | null>(null);
  const [missing, setMissing] = useState("");

  const bodyOf = () => ({
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
      onSuccess: (p) => {
        setFound(p);
        create.reset();
      },
    });
  }

  function add(e: FormEvent) {
    e.preventDefault();
    if (!found) return;
    setMissing("");
    const { token: t, token_username: u } = bodyOf();
    // What was found is what is added: the normalized address with the branch and folder the check resolved.
    create.mutate(
      {
        url: found.url,
        ...(found.ref ? { ref: found.ref } : {}),
        ...(found.path ? { path: found.path } : {}),
        ...(t ? { token: t } : {}),
        ...(u ? { token_username: u } : {}),
      } as const,
      { onSuccess: () => onOpenChange(false) },
    );
  }

  const urlProblem = repositoryAddressError(check.error, "template_url_invalid") ?? repositoryAddressError(create.error, "template_url_invalid") ?? missing;
  const unreachable = check.error instanceof ApiError && check.error.code === "template_unreachable";
  const failed = check.error ?? create.error;
  const showFailure = failed && !unreachable && !urlProblem;
  const busy = check.isPending || create.isPending;

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent
        title="Add a template repository"
        description={found ? "What Aurora found. Nothing is saved until you add the repository." : "Aurora reads the repository to see which templates it holds. It only ever reads."}
        size="xl"
        footer={
          <div className="flex flex-wrap justify-end gap-2">
            {/* Step 2 has no address field, so an address the add refused is said here, above Add repository. */}
            {found && urlProblem ? (
              <Notice tone="danger" role="alert" className="w-full">
                {urlProblem} Go back to change the address.
              </Notice>
            ) : null}
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
              {found ? "Add repository" : "Check"}
            </Button>
          </div>
        }
      >
        <form id={formId} onSubmit={found ? add : runCheck} className="space-y-4" noValidate>
          {/* `template_unreachable` says why the read failed, above the fields; a refused address says it on the field. */}
          {unreachable ? (
            <Notice tone="danger" role="alert">
              {errorMessage(check.error)}
            </Notice>
          ) : null}
          {showFailure ? (
            isCheckRateLimited(failed) ? (
              <Notice tone="danger" role="alert">
                {CHECK_RATE_LIMITED_SENTENCE}
              </Notice>
            ) : (
              <ErrorNotice error={failed} format={errorMessage} />
            )
          ) : null}
          {found ? (
            <Found found={found} />
          ) : (
            <RepositoryAddressFields idPrefix="tpl" url={url} onUrl={setUrl} branch={ref} onBranch={setRef} path={path} onPath={setPath} token={token} onToken={setToken} username={username} onUsername={setUsername} urlError={urlProblem || undefined} />
          )}
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** Step 2: what the read found (repository text as plain text) and each template with its state and problems. */
function Found({ found: f }: { found: TemplateSourcePreview }) {
  return (
    <div className="space-y-4" data-testid="template-source-found">
      {/* The dialog's own width decides the layout (container query), not the viewport. */}
      <div className="@container">
        <dl className="space-y-2 text-sm" aria-label="What Aurora found">
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Repository</dt>
            <dd className="min-w-0 font-mono text-xs text-ink [overflow-wrap:anywhere]">{f.repo}</dd>
          </div>
          {f.manifest.description ? (
            <div className="@md:flex @md:gap-4">
              <dt className="text-muted @md:w-32 @md:shrink-0">Description</dt>
              <dd className="min-w-0 text-ink [overflow-wrap:anywhere]">{f.manifest.description}</dd>
            </div>
          ) : null}
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Branch</dt>
            <dd className="min-w-0 text-ink [overflow-wrap:anywhere]">{resolvedRefWords(f.resolved)}</dd>
          </div>
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Latest commit</dt>
            <dd className="font-mono text-xs text-ink">{shortSha(f.commit_sha)}</dd>
          </div>
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Folder</dt>
            <dd className="min-w-0 text-ink [overflow-wrap:anywhere]">{f.path ? <span className="font-mono text-xs">{f.path}</span> : "Repository root"}</dd>
          </div>
          {/* Where the templates' packages come from: text, never fetched by the browser (C-1114). */}
          <div className="@md:flex @md:gap-4">
            <dt className="text-muted @md:w-32 @md:shrink-0">Packages come from</dt>
            <dd className="min-w-0 font-mono text-xs text-ink [overflow-wrap:anywhere]">{f.manifest.packages_url}</dd>
          </div>
        </dl>
      </div>
      <div className="space-y-2">
        <p className="text-sm font-medium text-ink">
          {f.templates.length} {f.templates.length === 1 ? "template" : "templates"}
        </p>
        <ul aria-label="Templates found" className="space-y-2">
          {f.templates.map((t) => (
            <li key={t.template_id} className="rounded-control border border-line px-3 py-2 text-sm">
              <p className="min-w-0 font-medium text-ink [overflow-wrap:anywhere]">{t.name}</p>
              <p className="min-w-0 text-xs text-ink-2 [overflow-wrap:anywhere]">{t.dir}</p>
              <p className="text-xs text-ink-2">
                {t.version ? `Version ${t.version}` : "No version yet"} · {foundStateWords(t.state)}
              </p>
              <p className="min-w-0 text-xs text-ink-2 [overflow-wrap:anywhere]">{t.summary}</p>
              {t.problems.length > 0 ? (
                <ul aria-label={`Problems of ${t.name}`} className="mt-1 space-y-0.5">
                  {t.problems.map((p, i) => (
                    <li key={`${p.path}-${i}`} className="text-xs text-danger-ink [overflow-wrap:anywhere]">
                      {problemWords(p)}
                    </li>
                  ))}
                </ul>
              ) : null}
            </li>
          ))}
        </ul>
      </div>
      {f.truncated ? <Notice role="status">This repository has more template folders than Aurora lists, so only the first were read. Choose a narrower folder to see the rest.</Notice> : null}
    </div>
  );
}
