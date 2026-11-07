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

export function timeZones(): string[] {
  const fn = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf;
  const zones = fn ? fn("timeZone") : [];
  return zones.includes("UTC") ? zones : ["UTC", ...zones];
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
