// The words of the four-step page that creates an agent from a template (P6-TPL-4 B1, C-1116).

/** An input of the template's content (C-1104's `InputDecl`), as the About step asks it. The content type keeps
 * `inputs` as `unknown[]` (C-1104 shapes pass through as stored), so this names the fields the page reads. */
export interface TemplateInput {
  id: string;
  label: string;
  help?: string | null;
  /** `text` (default) or `choice`. */
  kind?: string;
  required?: boolean;
  pattern?: string | null;
  max_length?: number | null;
  options?: string[] | null;
  default?: string | null;
}

export function asTemplateInputs(inputs: unknown): TemplateInput[] {
  return Array.isArray(inputs) ? (inputs as TemplateInput[]) : [];
}

/** Why an entry's app cannot be used here (C-1110's `unavailable_reason`), in one sentence. */
export function unavailableWords(app: string, reason: string | null | undefined): string {
  if (reason === "not_installed") return `Needs ${app}, which isn't installed here.`;
  return `Needs ${app}, which isn't available here.`;
}

/** A commit head as the seven characters people read (`sha7` everywhere else in the dashboard). */
export function sha7(sha: string | null | undefined): string {
  return (sha ?? "").slice(0, 7);
}

/**
 * Why a skill reference is not added (C-1116's Review step). A `library_not_added` reference names the library's
 * repository (C-1142): the required one gets C-1142's full sentence, an optional one C-1116's "Not added" form.
 * The other reasons keep the sentence that says why, without the repository.
 */
export function notAddedWords(ref: {
  name: string;
  required: boolean;
  repo?: string | null;
  unavailable_reason?: string | null;
}): string {
  if (ref.unavailable_reason === "library_not_added" && ref.repo) {
    return ref.required
      ? `Needs the skill library ${ref.repo}. An organization admin can add it.`
      : `Not added: needs the skill library ${ref.repo}.`;
  }
  const words = unavailableWords(`the ${ref.name} skill`, ref.unavailable_reason);
  return ref.required ? words : `Not added: ${words.charAt(0).toLowerCase()}${words.slice(1)}`;
}
