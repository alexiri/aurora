import { resolvedRefWords, repositoryAddressError } from "@/components/ui/repository-address";
import { type LibraryCheck } from "@/lib/api/skill-libraries";

// The address words are shared with the agent templates (C-1114); the library keeps its names.
export { CHECK_RATE_LIMITED_SENTENCE as RATE_LIMITED_SENTENCE, isCheckRateLimited as isRateLimited } from "@/components/ui/repository-address";

/** The sentence for a refused library address, or `null` when the error is not about the address (C-1122). */
export function addressError(err: unknown): string | null {
  return repositoryAddressError(err, "library_url_invalid");
}

/** "1 skill can be added" / "12 skills can be added". */
export function addableWords(valid: number): string {
  return `${valid} ${valid === 1 ? "skill" : "skills"} can be added`;
}

/** The resolved branch or tag of a library check, in words (C-1129). */
export function resolvedWords(c: LibraryCheck): string {
  // Resolved here by hand instead of a re-export so the type stays `LibraryCheck`.
  return resolvedRefWords(c.resolved);
}

