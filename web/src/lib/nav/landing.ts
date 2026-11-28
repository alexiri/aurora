import { adminRoutes } from "@/lib/routes";

export interface SlugItem {
  slug: string;
}

/** Prefer the `default` workspace, else the first one. */
export function pickDefaultWorkspace<W extends SlugItem>(workspaces: readonly W[]): W | undefined {
  return workspaces.find((w) => w.slug === "default") ?? workspaces[0];
}

/** Landing path after sign-in: first org's default workspace; null when there is no org. */
export function landingPath(
  orgs: readonly SlugItem[],
  workspaces: readonly SlugItem[] | undefined,
): string | null {
  const org = orgs[0];
  if (!org) return null;
  const ws = workspaces && pickDefaultWorkspace(workspaces);
  return ws ? `/o/${org.slug}/w/${ws.slug}` : `/o/${org.slug}`;
}

interface MeLike {
  memberships: readonly { org: SlugItem & { name?: string }; role?: string; status: string; workspaces: readonly (SlugItem & { personal?: boolean })[] }[];
}

/** The person's own personal workspace (D-140), as a path, when they have one in an active membership. */
export function personalWorkspacePath(me: MeLike | undefined): string | null {
  for (const m of me?.memberships ?? []) {
    if (m.status !== "active") continue;
    const w = m.workspaces.find((x) => x.personal === true);
    if (w) return `/o/${m.org.slug}/w/${w.slug}`;
  }
  return null;
}

/**
 * (D-508(e)) `/` must never send a member to Manage <org> (an organization's own address): Manage would send them
 * straight back. A landing there becomes one of their workspaces; `null` when they have none anywhere.
 */
function openable(landing: string, me: MeLike | undefined, isInstanceAdmin: boolean): string | null {
  const manage = landing.match(/^\/o\/([^/]+)$/);
  if (!manage || isInstanceAdmin) return landing;
  if (me?.memberships.find((m) => m.org.slug === manage[1])?.role !== "member") return landing;
  const own = personalWorkspacePath(me);
  if (own) return own;
  for (const m of me.memberships) {
    if (m.status === "active" && m.workspaces[0]) return `/o/${m.org.slug}/w/${m.workspaces[0].slug}`;
  }
  return null;
}

/**
 * Where `/` sends a person (`14 §Onboarding`): an instance admin whose instance has no saved placement goes to the
 * setup wizard; a person who has not seen the Welcome page goes to it, in their personal workspace when they have one;
 * everyone else lands on the workspace Overview.
 */
export function homeTarget(args: {
  landing: string | null;
  me: MeLike | undefined;
  isInstanceAdmin: boolean;
  placementSaved: boolean | undefined;
  welcomeSeen: boolean | undefined;
}): string | null {
  const { me, isInstanceAdmin, placementSaved, welcomeSeen } = args;
  if (isInstanceAdmin && placementSaved === false) return adminRoutes.home();
  const landing = args.landing ? openable(args.landing, me, isInstanceAdmin) : null;
  if (!landing) return null;
  if (welcomeSeen === false) {
    const personal = personalWorkspacePath(me);
    if (personal) return `${personal}/welcome`;
    if (landing.includes("/w/")) return `${landing}/welcome`;
  }
  return landing;
}

export function timeZones(): string[] {
  const fn = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf;
  const zones = fn ? fn("timeZone") : [];
  return zones.includes("UTC") ? zones : ["UTC", ...zones];
}

/** A memory language as a person reads it ("english" → "English"; the search config "simple" is any other language). */
export function memoryLanguageName(language: string): string {
  const l = memoryLanguageConfig(language);
  if (l === "simple") return "Any other language";
  return l.charAt(0).toUpperCase() + l.slice(1);
}

const LANGUAGE_CODES: Record<string, string> = {
  en: "english", de: "german", fr: "french", es: "spanish", it: "italian", pt: "portuguese",
  nl: "dutch", sv: "swedish", no: "norwegian", nb: "norwegian", da: "danish", fi: "finnish", ru: "russian",
};

/** A stored language as the search config the list offers ("en" → "english"); anything else comes back unchanged. */
export function memoryLanguageConfig(language: string): string {
  return LANGUAGE_CODES[language.toLowerCase()] ?? language;
}

export const MEMORY_LANGUAGES = [
  "english",
  "german",
  "french",
  "spanish",
  "italian",
  "portuguese",
  "dutch",
  "swedish",
  "norwegian",
  "danish",
  "finnish",
  "russian",
  "simple",
] as const;
