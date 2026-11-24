"use client";

import { Fold } from "@/components/ui/fold";
import { Field, inputClass } from "@/components/ui/field";
import { SecretInput } from "@/components/ui/secret-input";
import { ApiError } from "@/lib/api/client";
import { shortSha } from "@/lib/api/skill-libraries";

/**
 * The repository-address fields one https git repository is named by (C-1122, C-1123): address,
 * branch or tag, folder, and the private fold with the access token and its user name. One form for
 * every feature that reads a repository (skill libraries C-1129, agent templates C-1114), so there
 * is never a second address form. The caller owns the state; nothing here parses the address (the
 * server does, C-1123). Every group renders only when its change handler is given: the Add flows use
 * all of them, an Edit flow that cannot change the address omits it, and one with its own token
 * replace/remove row omits the fold.
 */
export function RepositoryAddressFields({
  idPrefix,
  url,
  onUrl,
  ref,
  onRef,
  path,
  onPath,
  token,
  onToken,
  username,
  onUsername,
  urlError,
  urlHint,
  pathHint = "The folder that holds the skill folders. Default: the repository root",
}: {
  idPrefix: string;
  url?: string;
  onUrl?: (v: string) => void;
  ref?: string;
  onRef?: (v: string) => void;
  path?: string;
  onPath?: (v: string) => void;
  token?: string;
  onToken?: (v: string) => void;
  username?: string;
  onUsername?: (v: string) => void;
  /** The sentence for a refused address, shown on the address field (C-1122's field error). */
  urlError?: string;
  urlHint?: string;
  pathHint?: string;
}) {
  return (
    <>
      {onUrl ? (
        <Field
          id={`${idPrefix}-url`}
          label="Repository address"
          error={urlError || undefined}
          hint={urlHint ?? "The address you clone, or the repository's page. For example https://gitlab.example.org/team/skills or https://github.com/acme/skills/tree/main/skills"}
        >
          <input
            id={`${idPrefix}-url`}
            className={inputClass}
            value={url ?? ""}
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => onUrl(e.target.value)}
          />
        </Field>
      ) : null}
      {onRef ? (
        <Field id={`${idPrefix}-ref`} label="Branch or tag" hint="Default: the repository's default branch">
          <input id={`${idPrefix}-ref`} className={inputClass} value={ref ?? ""} autoComplete="off" spellCheck={false} onChange={(e) => onRef(e.target.value)} />
        </Field>
      ) : null}
      {onPath ? (
        <Field id={`${idPrefix}-path`} label="Folder" hint={pathHint}>
          <input id={`${idPrefix}-path`} className={inputClass} value={path ?? ""} autoComplete="off" spellCheck={false} onChange={(e) => onPath(e.target.value)} />
        </Field>
      ) : null}
      {onToken ? (
        <Fold summary="This repository is private" defaultOpen={!!token}>
          <div className="mt-3 space-y-4">
            <Field id={`${idPrefix}-token`} label="Access token">
              <SecretInput id={`${idPrefix}-token`} value={token ?? ""} onChange={onToken} aria-label="Access token" />
            </Field>
            {onUsername ? (
              <Field id={`${idPrefix}-user`} label="User name" hint="Most hosts accept any name. Bitbucket: x-token-auth">
                <input
                  id={`${idPrefix}-user`}
                  className={inputClass}
                  value={username ?? ""}
                  autoComplete="off"
                  spellCheck={false}
                  onChange={(e) => onUsername(e.target.value)}
                />
              </Field>
            ) : null}
            <p className="text-xs text-muted">Use a token that can only read this repository. Aurora only reads.</p>
          </div>
        </Fold>
      ) : null}
    </>
  );
}

/** One sentence per `details.reason` of a 422 `*_url_invalid` (C-1122, C-1123; templates: C-1107). */
const URL_REASON: Record<string, string> = {
  not_https: "Use the https address; it starts with https://.",
  ssh_url: "Use the https address; Aurora doesn't use SSH.",
  credentials_in_url: "Take the user name and password out of the address. For a private repository use the access token below.",
  query_or_fragment: "Take everything after ? or # out of the address.",
  not_a_repository_path: "That address doesn't lead to a repository. Use the repository's address or its page.",
  too_long: "That address is too long. Use the repository's own address.",
};

/**
 * The sentence for a refused repository address, or `null` when the error is not about the address.
 * `code` is the feature's error code: `library_url_invalid` (C-1122) or `template_url_invalid` (C-1107).
 */
export function repositoryAddressError(err: unknown, code: "library_url_invalid" | "template_url_invalid"): string | null {
  if (!(err instanceof ApiError) || err.code !== code) return null;
  const reason = (err.details as { reason?: unknown } | null)?.reason;
  return (
    (typeof reason === "string" ? URL_REASON[reason] : undefined) ??
    "That isn't a repository address Aurora can use. Paste the https address of the repository, or its page in a browser."
  );
}

/** The check endpoints answer 10 per minute per person; the answer says when to try again. */
export function isCheckRateLimited(err: unknown): boolean {
  return err instanceof ApiError && (err.code === "rate_limited" || err.status === 429);
}
export const CHECK_RATE_LIMITED_SENTENCE = "Aurora checks ten repositories a minute. Try again in a minute.";

/** What a check resolved the branch or tag to: "main (default branch)", "Tag v1.2" or "Commit 0123456". */
export function resolvedRefWords(resolved: { kind: string; name: string; default: boolean } | null | undefined): string {
  if (!resolved) return "";
  if (resolved.kind === "tag") return `Tag ${resolved.name}`;
  if (resolved.kind === "commit") return `Commit ${shortSha(resolved.name)}`;
  return resolved.default ? `${resolved.name} (default branch)` : resolved.name;
}
