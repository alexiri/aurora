import type { Account, AgentAccess, Level } from "@/lib/api/accounts";

/** `14 §2`: the three choices, in the fixed order, with the labels people read everywhere. */
export const LEVEL_LABEL: Record<Level, string> = {
  read: "Read only",
  ask: "Ask before changes",
  act: "Change without asking",
};

export const LEVEL_ORDER: readonly Level[] = ["read", "ask", "act"];

/** (P5-REFINE-13, D-419, C-1067) A send-only app (the Outgoing webhook) has two levels of its own; Read only is never offered. */
export const SEND_LEVEL_LABEL: Partial<Record<Level, string>> = {
  ask: "Ask before each send",
  act: "Send without asking",
};

/** The label of a level, in the words of a send-only app when `sendOnly`. */
export function levelLabel(level: Level, sendOnly = false): string {
  return (sendOnly ? SEND_LEVEL_LABEL[level] : undefined) ?? LEVEL_LABEL[level];
}

/** The sentence of each choice (`14 §2`), for an agent named `agent`. */
export function levelSentence(level: Level, agent: string, sendOnly = false): string {
  if (sendOnly && level === "ask") return `${agent} asks you before it sends anything. You answer in Approvals.`;
  if (sendOnly && level === "act") return `${agent} may send on its own.`;
  switch (level) {
    case "read":
      return `${agent} can look things up but never change anything.`;
    case "ask":
      return `${agent} asks you before each change. You answer in Approvals.`;
    case "act":
      return `${agent} may make changes on its own.`;
  }
}

/** What one agent's entry says in the row: "Daneel · Read only". */
export function entryText(entry: AgentAccess, account?: Pick<Account, "ref" | "levels"> & Partial<Pick<Account, "app">>): string {
  switch (entry.level) {
    case "read":
    case "ask":
    case "act":
      return `${entry.agent.name} · ${levelLabel(entry.level, account?.app?.send_only === true)}`;
    case "custom":
      return `${entry.agent.name} · Custom`;
    case "use":
      // A read-only app has no levels and the grant level `use`: the pill still says "Read only".
      return account?.ref.type === "connection" && account.levels.length === 0 ? `${entry.agent.name} · Read only` : entry.agent.name;
  }
}

/**
 * (P5-REFINE-13, access-11) What an app account's menu offers. The API does not say which apps can be tested or signed
 * in again, so the rule is derived from the app's connect methods: **Test the connection** where a method has a probe
 * (`has_probe`); **Sign in again** where a method is OAuth (`kind: "oauth2"`); the Outgoing webhook (`app_id: "webhook"`)
 * has **Send a test** instead of a probe. With the app unknown (not loaded, not installed) none is offered.
 */
export function accountActions(
  appId: string | undefined,
  methods: ReadonlyArray<{ kind: string; has_probe?: boolean }> | undefined,
): { test: boolean; signInAgain: boolean; sendTest: boolean } {
  if (appId === "webhook") return { test: false, signInAgain: false, sendTest: true };
  const list = methods ?? [];
  return { test: list.some((m) => m.has_probe === true), signInAgain: list.some((m) => m.kind === "oauth2"), sendTest: false };
}

/** The label of an agent in a picker: "<workspace> · <agent>" when agents of more than one workspace are listed (C-948). */
export function agentLabel(agent: { name: string }, workspace: { name: string }, several: boolean): string {
  return several ? `${workspace.name} · ${agent.name}` : agent.name;
}

export function kindLabel(kind: Account["kind"]): string {
  return kind === "app" ? "App" : kind === "website" ? "Website" : "AI key";
}

/** The choices an account offers, cut to what is known (the server already cut them to the limit). */
export function hasChoice(account: Pick<Account, "levels">): boolean {
  return account.levels.length > 0;
}

/** The choices are cut by an organization's limit (not by a `custom_only` app, which has no "act" for another reason). */
export function isLimited(account: Pick<Account, "levels" | "limit" | "app">): boolean {
  return account.limit !== undefined && account.limit !== null && account.levels.length > 0 && account.limit !== "act";
}

/**
 * Where **Connect** on the organization's Apps tab can connect (C-947): the person's own and shared workspaces lead,
 * other people's personal workspaces are folded away. `me` is the person's user id.
 */
export function splitWorkspaces<T extends { personal_of?: { id: string } | null }>(items: T[], me: string | undefined): { mine: T[]; others: T[] } {
  const others = items.filter((w) => !!w.personal_of && w.personal_of.id !== me);
  return { mine: items.filter((w) => !others.includes(w)), others };
}

/** What an agent's access reads as in the lowering confirmation: a choice, "Custom", or nothing left (C-990). */
function loweredLabel(level: string): string {
  return level in LEVEL_LABEL ? LEVEL_LABEL[level as Level] : level === "none" ? "No access" : "Custom";
}

/**
 * The confirmation before a limit is lowered (C-937, C-990): one sentence per kind of change, "3 agents go from Change
 * without asking to Read only". An empty list says nothing changes.
 */
export function loweringSentences(lowered: ReadonlyArray<{ from: string; to: string }>): string[] {
  const counts = new Map<string, { n: number; from: string; to: string }>();
  for (const l of lowered) {
    const key = `${l.from}>${l.to}`;
    const c = counts.get(key);
    if (c) c.n += 1;
    else counts.set(key, { n: 1, from: l.from, to: l.to });
  }
  return [...counts.values()].map((c) => `${c.n} ${c.n === 1 ? "agent goes" : "agents go"} from ${loweredLabel(c.from)} to ${loweredLabel(c.to)}`);
}

/** The agents that can use the account first-hand are the ones listed; "Not given" are the rest of an agent's view. */
export function splitGiven(accounts: Account[]): { given: Account[]; notGiven: Account[] } {
  return { given: accounts.filter((a) => a.given !== false), notGiven: accounts.filter((a) => a.given === false) };
}

/** The Ask-before-changes preselection rule of a picker (C-925): the contextual agent, else the only one, else none. */
export function preselectAgents<T extends { id: string }>(agents: T[], contextualId?: string | null): string[] {
  if (contextualId && agents.some((a) => a.id === contextualId)) return [contextualId];
  if (agents.length === 1) return [agents[0].id];
  return [];
}
