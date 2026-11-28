/**
 * (P5-SKILLS-3, D-498, D-499, D-505, D-506) The one list of skills, as data: what an agent can do (its effective set,
 * each name once) and what a workspace or an organization holds, with the words every row and panel uses.
 */
import type { components } from "@/lib/api/schema";
import type { BudgetTier } from "@/lib/api/skill-libraries";
import type { EffectiveSkill, ShadowedSkill, Skill, SkillScope } from "@/lib/api/skills";
import { skillDisplayName } from "@/lib/skill-name";

/** A skill as any route sends it; an app skill has no id. */
export type SkillBody = components["schemas"]["EffectiveBody"];
export type SkillUsage = components["schemas"]["SkillUsage"];
export type AppRef = components["schemas"]["AppRef"];

export type RowTier = BudgetTier | "app";

/** Which page the list is on, with the names its words use. */
export type SkillPage =
  | {
      kind: "agent";
      agentId: string;
      agentName: string;
      orgId: string | undefined;
      orgName: string;
      /** For Try it in chat: the agent's address. Null leaves the button out. */
      chat: ((draft: string) => string) | null;
      /** Emergency read access: skills can be read, not written. */
      readOnly: boolean;
      /** Who may decide a library update, per tier. */
      canDecide: Record<BudgetTier, boolean>;
    }
  | {
      kind: "workspace";
      wsId: string;
      orgId: string | undefined;
      label: string;
      canOwn: boolean;
      /** The viewer reads this workspace through their own emergency grant (an agent's page says it with `readOnly`). */
      emergencyReader?: boolean;
    }
  | { kind: "organization"; orgId: string; label: string };

/**
 * An agent's page for an emergency reader: it holds the agent's own skills and nothing of what the agent inherits, so
 * it must draw nothing that would read as "nothing is inherited".
 */
export function ownSkillsOnly(page: SkillPage): boolean {
  return page.kind === "agent" && page.readOnly;
}

export const emergencyOwnSkillsWords = (agentName: string) => `Emergency read access shows only ${agentName}’s own skills.`;

export function pageScope(page: SkillPage): SkillScope {
  if (page.kind === "agent") return { kind: "agent", id: page.agentId };
  return page.kind === "workspace" ? { kind: "workspace", id: page.wsId } : { kind: "organization", id: page.orgId };
}

/** The level a page itself writes to (never the app tier). */
export function pageTier(page: SkillPage): BudgetTier {
  return page.kind === "organization" ? "org" : page.kind;
}

/** Whom the switch column and the switch's name are about: the agent, the workspace or the organization. */
export function pageSubject(page: SkillPage): string {
  return page.kind === "agent" ? page.agentName : page.label;
}

export interface SkillRow {
  /** What `?skill=` holds. */
  key: string;
  skill: SkillBody;
  tier: RowTier;
  app: AppRef | null;
  name: string;
  /** Whether the skill applies here (the row's switch). */
  on: boolean;
  /**
   * What the row's switch changes: this skill (`own`: the agent's own, or the level's), this agent only (`inherited`),
   * or nothing (`app`: apps' skills come with the app; `fixed`: switched off by its owners, or read-only).
   */
  control: "own" | "inherited" | "app" | "fixed";
  /** Why a fixed switch cannot be used. */
  fixedWhy: string | null;
  usage: SkillUsage | null;
  /** The skill of the same name this one replaces, for the Compare line. */
  replaces: { skill: SkillBody; tier: RowTier; app: AppRef | null } | null;
}

const RANK: Record<RowTier, number> = { agent: 0, workspace: 1, org: 2, app: 3 };

const asTier = (tier: string): RowTier => (tier === "agent" || tier === "workspace" || tier === "app" ? tier : "org");

export const rowKey = (skill: SkillBody, app: AppRef | null) => skill.id ?? `app:${app?.app_id ?? ""}:${skill.name}`;

export function rowName(row: Pick<SkillRow, "skill">): string {
  return skillDisplayName(row.skill.name, row.skill.title);
}

/** Library content that still follows its library: read-only, pinned. */
export function isLibrary(skill: SkillBody): boolean {
  return !!skill.source && !skill.source.detached;
}

/**
 * Text an outsider wrote: an app's, or a library's (also after its updates were stopped, which keeps the text). Showing
 * it must fetch nothing. A skill written in Aurora shows its pictures as chat and Memory do.
 */
export function isOutsideText(skill: SkillBody): boolean {
  return skill.tier === "app" || !!skill.source;
}

/** What the viewer may do with a skill on this page. */
export function skillRights(page: SkillPage, row: SkillRow) {
  // Whether this page is where the skill is kept, and the viewer may write there.
  const here =
    page.kind === "agent" ? row.tier === "agent" && !page.readOnly : row.tier === pageTier(page) && (page.kind === "organization" || page.canOwn);
  const library = isLibrary(row.skill);
  const canDecide = row.tier === "app" ? false : page.kind === "agent" ? page.canDecide[row.tier] : here;
  return { here, library, canDecide, edit: here && !library };
}

/**
 * Whether an update of this row's skill waits for this person: the one rule for the row's "Update available", the
 * panel's Review update and the page's notice, so the three always agree.
 */
export function updateToReview(page: SkillPage, row: SkillRow): boolean {
  const rights = skillRights(page, row);
  return rights.library && rights.canDecide && !!row.skill.source?.update_pending && !!row.skill.id;
}

export function libraryLabel(skill: SkillBody): string {
  return skill.source?.label ?? skill.source?.repo ?? "a library";
}

const sortRows = (rows: SkillRow[]) =>
  rows.sort((a, b) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || RANK[a.tier] - RANK[b.tier]);

/** "Acme’s admins" / "the workspace’s owners": who manages a level's skills. */
export function managers(tier: RowTier, orgName: string): string {
  return tier === "org" ? `${orgName}’s admins` : "the workspace’s owners";
}

/**
 * An agent's one list (the effective route's two answers): every skill that applies, and every one that is off, except
 * where a higher level's skill of the same name applies anyway. A replaced skill is not a row; the row that replaces it
 * says so.
 */
export function agentRows(
  eff: { items: EffectiveSkill[]; shadowed: ShadowedSkill[] },
  opts: { orgName: string; readOnly: boolean },
): SkillRow[] {
  const reasonOf = (s: ShadowedSkill) => s.reason ?? (s.enabled ? "replaced" : "disabled");
  const row = (e: EffectiveSkill | ShadowedSkill, on: boolean): SkillRow => {
    const tier = asTier(e.tier);
    const skill = e.skill;
    const app = e.app ?? null;
    let control: SkillRow["control"] = tier === "agent" ? "own" : tier === "app" ? "app" : "inherited";
    let fixedWhy: string | null = null;
    // Only off is possible for one agent: what its owners switched off stays off (D-499(b)).
    if (control === "inherited" && !on && !e.enabled) {
      control = "fixed";
      fixedWhy = `Turned off by ${managers(tier, opts.orgName)}`;
    }
    if (opts.readOnly && control !== "app") control = "fixed";
    return { key: rowKey(skill, app), skill, tier, app, name: skillDisplayName(skill.name, skill.title), on, control, fixedWhy, usage: e.usage ?? null, replaces: null };
  };
  const rows = eff.items.map((i) => {
    const r = row(i, true);
    const lower = eff.shadowed
      .filter(
        (s) =>
          s.skill.name === i.skill.name &&
          reasonOf(s) === "replaced" &&
          (!s.shadowed_by || (i.skill.id ? s.shadowed_by.id === i.skill.id : s.shadowed_by.app_id === i.app?.app_id)),
      )
      .sort((a, b) => RANK[asTier(a.tier)] - RANK[asTier(b.tier)])[0];
    if (lower) r.replaces = { skill: lower.skill, tier: asTier(lower.tier), app: lower.app ?? null };
    return r;
  });
  for (const s of eff.shadowed) {
    if (reasonOf(s) === "replaced") continue;
    const outranked = eff.items.some((i) => i.skill.name === s.skill.name && RANK[asTier(i.tier)] < RANK[asTier(s.tier)]);
    if (!outranked) rows.push(row(s, false));
  }
  return sortRows(rows);
}

/** A workspace's or an organization's own skills; also an agent's own for an emergency reader. */
export function levelRows(skills: Skill[], writable: boolean): SkillRow[] {
  return sortRows(
    skills.map((skill) => ({
      key: skill.id,
      skill,
      tier: asTier(skill.tier),
      app: null,
      name: skillDisplayName(skill.name, skill.title),
      on: skill.enabled,
      control: writable ? ("own" as const) : ("fixed" as const),
      fixedWhy: null,
      usage: null,
      replaces: null,
    })),
  );
}

// ---- chips and search ---------------------------------------------------------------------------

export type SkillFilter = "all" | "own" | "workspace" | "org" | "written" | "libraries" | "off";

export function matchesFilter(row: SkillRow, filter: SkillFilter): boolean {
  switch (filter) {
    case "own":
      return row.tier === "agent";
    case "workspace":
      return row.tier === "workspace";
    case "org":
      return row.tier === "org";
    case "written":
      return !isLibrary(row.skill);
    case "libraries":
      return isLibrary(row.skill);
    case "off":
      return !row.on;
    default:
      return true;
  }
}

export function filterOptions(page: SkillPage, rows: SkillRow[]): { value: SkillFilter; label: string }[] {
  const chip = (value: SkillFilter, words: string) => ({ value, label: `${words} ${rows.filter((r) => matchesFilter(r, value)).length}` });
  if (page.kind === "agent") {
    const inherited = ownSkillsOnly(page) ? [] : [chip("workspace", "From the workspace"), chip("org", `From ${page.orgName}`)];
    return [chip("all", "All"), chip("own", `${page.agentName}’s own`), ...inherited, chip("off", "Off")];
  }
  return [chip("all", "All"), chip("written", "Written here"), chip("libraries", "From libraries"), chip("off", "Off")];
}

export function matchesSearch(row: SkillRow, q: string): boolean {
  const needle = q.trim().toLowerCase();
  return !needle || [row.name, row.skill.name, row.skill.description].some((s) => s.toLowerCase().includes(needle));
}

// ---- words --------------------------------------------------------------------------------------

export type SourceTone = "agent" | "workspace" | "org" | "library" | "app";

/** The one source chip of a row. */
export function sourceChip(page: SkillPage, row: SkillRow): { label: string; tone: SourceTone } {
  const library = isLibrary(row.skill);
  const label = libraryLabel(row.skill);
  if (page.kind !== "agent") {
    if (library) return { label: `Library · ${label}`, tone: "library" };
    return { label: "Written here", tone: row.tier === "org" ? "org" : "workspace" };
  }
  if (row.tier === "app") return { label: "App", tone: "app" };
  if (row.tier === "agent") return library ? { label: `Library · ${label}`, tone: "library" } : { label: page.agentName, tone: "agent" };
  const tone = row.tier === "workspace" ? "workspace" : "org";
  const level = tone === "workspace" ? "Workspace" : page.orgName;
  return { label: library ? `${level} · Library` : level, tone };
}

/** "the workspace’s", "Acme’s", "the app’s": a replaced skill, by its level. */
function whose(tier: RowTier, orgName: string, app: AppRef | null): string {
  if (tier === "workspace") return "the workspace’s";
  if (tier === "org") return `${orgName}’s`;
  if (tier === "app") return app ? `the ${app.name} app’s` : "the app’s";
  return "the agent’s";
}

/** "Daneel’s version replaces the workspace’s". */
export function replaceWords(page: Extract<SkillPage, { kind: "agent" }>, row: SkillRow): string | null {
  if (!row.replaces) return null;
  const winner =
    row.tier === "agent" ? `${page.agentName}’s` : row.tier === "workspace" ? "The workspace’s" : row.tier === "org" ? `${page.orgName}’s` : `The ${row.app?.name ?? ""} app’s`;
  return `${winner} version replaces ${whose(row.replaces.tier, page.orgName, row.replaces.app)}`;
}

/** "Release notes, Daneel’s own version, wins over the workspace’s." under the list. */
export function overrideSentence(page: SkillPage, rows: SkillRow[]): string | null {
  if (page.kind !== "agent") return null;
  const replacing = rows.filter((r) => r.replaces);
  if (replacing.length !== 1) return null;
  const r = replacing[0]!;
  const winner = r.tier === "agent" ? `${page.agentName}’s own version` : r.tier === "workspace" ? "the workspace’s version" : r.tier === "org" ? `${page.orgName}’s version` : "the app’s version";
  return `${r.name}, ${winner}, wins over ${whose(r.replaces!.tier, page.orgName, r.replaces!.app)}.`;
}

export function combineWords(page: SkillPage): string {
  if (page.kind === "agent") {
    return `One skill per name. ${page.agentName}’s own skill replaces a workspace skill, which replaces one of ${page.orgName}’s, which replaces an app’s. A skill that is off replaces nothing.`;
  }
  if (page.kind === "organization") {
    return "A workspace skill or an agent's own skill with the same name overrides an organization skill for that workspace or agent. Workspace owners manage workspace skills on each workspace's Skills page.";
  }
  return "Skills come from your organization and from this workspace. If both have a skill with the same name, the workspace's version is used. New and changed skills reach your agents shortly.";
}

export const usedByWords = (n: number) => `Used by ${n} ${n === 1 ? "agent" : "agents"}`;

const times = (n: number) => `${n} ${n === 1 ? "time" : "times"}`;

/**
 * The panel's usage line (C-1175). On an agent: its own use this week and the last one, naming the conversation only
 * when the server did (it does only for people who may read it); for an inherited skill also how many agents use it.
 * Null where nothing is counted (an app skill).
 */
export function usageWords(page: SkillPage, row: SkillRow): string | null {
  const usedBy = row.skill.used_by_agents;
  if (page.kind !== "agent") return typeof usedBy === "number" ? usedByWords(usedBy) : null;
  const usage = row.usage;
  if (!usage) return null;
  if ((row.tier === "workspace" || row.tier === "org") && typeof usedBy === "number") {
    const where = row.tier === "org" ? page.orgName : "this workspace";
    return `${usedByWords(usedBy)} in ${where} · ${page.agentName} used it ${times(usage.week)} this week`;
  }
  if (!usage.last && usage.week === 0) return "Not used yet";
  const title = usage.last?.conversation?.title?.trim();
  const last = usage.last ? ` · last in ${title ? `“${title}”` : "a conversation"}` : "";
  return `Used ${times(usage.week)} this week${last}`;
}
