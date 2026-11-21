import { ApiError } from "@/lib/api/client";
import type { LibraryCheck } from "@/lib/api/skill-libraries";
import { shortSha } from "@/lib/api/skill-libraries";

/** One sentence per `details.reason` of a 422 `library_url_invalid` (C-1122, C-1123). */
const URL_REASON: Record<string, string> = {
  not_https: "Use the https address; it starts with https://.",
  ssh_url: "Use the https address; Aurora doesn't use SSH.",
  credentials_in_url: "Take the user name and password out of the address. For a private repository use the access token below.",
  query_or_fragment: "Take everything after ? or # out of the address.",
  not_a_repository_path: "That address doesn't lead to a repository. Use the repository's address or its page.",
  too_long: "That address is too long. Use the repository's own address.",
};

/** The sentence for a refused address, or `null` when the error is not about the address. */
export function addressError(err: unknown): string | null {
  if (!(err instanceof ApiError) || err.code !== "library_url_invalid") return null;
  const reason = (err.details as { reason?: unknown } | null)?.reason;
  return (typeof reason === "string" ? URL_REASON[reason] : undefined) ?? "That isn't a repository address Aurora can use. Paste the https address of the repository, or its page in a browser.";
}

/** The check endpoint answers 10 per minute per person; the answer says when to try again. */
export function isRateLimited(err: unknown): boolean {
  return err instanceof ApiError && (err.code === "rate_limited" || err.status === 429);
}
export const RATE_LIMITED_SENTENCE = "Aurora checks ten repositories a minute. Try again in a minute.";

/** "main (default branch)", "Tag v1.2" or "Commit 0123456": what the check resolved the branch or tag to. */
export function resolvedWords(c: LibraryCheck): string {
  const r = c.resolved;
  if (r.kind === "tag") return `Tag ${r.name}`;
  if (r.kind === "commit") return `Commit ${shortSha(r.name)}`;
  return r.default ? `${r.name} (default branch)` : r.name;
}

/** "1 skill can be added" / "12 skills can be added". */
export function addableWords(valid: number): string {
  return `${valid} ${valid === 1 ? "skill" : "skills"} can be added`;
}
