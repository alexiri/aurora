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
