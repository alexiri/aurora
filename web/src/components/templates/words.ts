import { label, TEMPLATE_FOUND_STATE, TEMPLATE_VERSION_STATUS } from "@/lib/labels";

/** One refusal the repository's files caused (C-1141): `{ code, path, message }`; a check names a `dir` instead. */
export interface TemplateProblemLike {
  code?: unknown;
  message?: unknown;
  path?: unknown;
  dir?: unknown;
}

/** The words for one problem: where it applies (the file, or the check's directory) and the reader's sentence. */
export function problemWords(p: TemplateProblemLike): string {
  const message = typeof p.message === "string" ? p.message : "";
  const where = typeof p.path === "string" ? p.path : typeof p.dir === "string" ? p.dir : null;
  return where ? `${where}: ${message}` : message;
}

/** The words for a template found in a repository (`new`, `id_taken`, …). */
export function foundStateWords(state: string | null | undefined): string {
  return label(TEMPLATE_FOUND_STATE, state);
}

/** The words for a stored version's state (`awaiting_package`, `pending_approval`, …). */
export function versionStatusWords(status: string | null | undefined): string {
  return label(TEMPLATE_VERSION_STATUS, status);
}
