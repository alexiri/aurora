import { useApps } from "@/lib/api/apps";
import { appDisplayName } from "@/lib/api/app-name";
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

/** The name of an app in the catalogue; an app Aurora does not know keeps its id (it may not be installed here). */
export function useAppName() {
  const { data } = useApps(undefined);
  const items = data?.items ?? [];
  return (appId: string) => {
    const a = items.find((x) => x.app_id === appId);
    return a ? appDisplayName(a) : appId;
  };
}

// ---- The gallery and the details view (C-1116) -------------------------------------------------

type CardLike = {
  apps: { app_ids: string[]; available: boolean; required: boolean }[];
  skill_refs: {
    index: number;
    kind: string;
    name: string;
    required: boolean;
    available: boolean;
    repo?: string | null;
  }[];
  ready: boolean;
};

/** Why a template is not ready, one line per missing thing (C-1116): the apps first, then the skill references. */
export function notReadyWords(card: CardLike, appName: (id: string) => string): string[] {
  const out: string[] = [];
  for (const entry of card.apps) {
    if (entry.required && !entry.available) out.push(`Needs ${appName(entry.app_ids[0] ?? "")}, which isn't available here.`);
  }
  for (const ref of card.skill_refs) {
    if (ref.required && !ref.available) {
      if (ref.kind === "organization") out.push(`Needs your organization's ${ref.name} skill.`);
      // The card names the library's repository (C-1116's "Needs the skill library <repo>"); the
      // API normalizes it (C-1142). Without one the line still names the skill to look for.
      else if (ref.repo) out.push(`Needs the skill library ${ref.repo}.`);
      else out.push(`Needs the skill library for the ${ref.name} skill.`);
    }
  }
  return out;
}
