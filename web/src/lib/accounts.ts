import type { Account, AgentAccess, Level } from "@/lib/api/accounts";

/** `14 §2`: the three choices, in the fixed order, with the labels people read everywhere. */
export const LEVEL_LABEL: Record<Level, string> = {
  read: "Read only",
  ask: "Ask before changes",
  act: "Change without asking",
};

export const LEVEL_ORDER: readonly Level[] = ["read", "ask", "act"];

/** The sentence of each choice (`14 §2`), for an agent named `agent`. */
export function levelSentence(level: Level, agent: string): string {
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
export function entryText(entry: AgentAccess, account?: Pick<Account, "ref" | "levels">): string {
  switch (entry.level) {
    case "read":
    case "ask":
    case "act":
      return `${entry.agent.name} · ${LEVEL_LABEL[entry.level]}`;
    case "custom":
      return `${entry.agent.name} · Custom`;
    case "use":
      // A read-only app has no levels and the grant level `use`: the pill still says "Read only".
      return account?.ref.type === "connection" && account.levels.length === 0 ? `${entry.agent.name} · Read only` : entry.agent.name;
  }
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
