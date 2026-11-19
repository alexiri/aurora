import type { Account } from "@/lib/api/accounts";
import { healthClause, needsAttention } from "@/lib/account-health";

import { askSentence, reachSentence, type InternetAccess } from "./internet";

/**
 * (P5-UX-7, C-986, D-350) What one agent can do, in a few sentences computed from its accounts and the workspace's
 * Internet access: exactly the table of `14 §8`, in its order, with no wording of its own. Pure; the unit tests walk
 * the table. Every sentence must be true under `07 §Evaluation`:
 *
 * - An account an agent can use is *given* to it (a list read with `agent_id`: `given !== false`) and *working* (an
 *   account that needs attention is said in its own sentence and in no other: the agent cannot use it right now).
 * - `read`, `ask` and `act` all allow reads, as does `use` (an app with no choice is read only, a website is reached
 *   with its token); so those accounts are "can read". `custom` is any choice that is not a level: it may not allow
 *   reads, so it is said as "custom access" and never as "can read".
 * - `ask` asks before every change except the ones saved as always allowed; `act` makes changes without asking.
 * - An AI key is not an app: it is not listed and counts for nothing here.
 * - The website sentences are the workspace's (a request without an account); the reach sentence is always said and
 *   the ask sentence only when a rule asks (`internet.ts`). When the viewer cannot read them (`internet: null`) the
 *   summary says nothing about websites rather than guess.
 */

export interface AgentSummaryInput {
  /** The agent's name. */
  agent: string;
  /** The agent's id: the entry of each account's `agents` that is this agent's. */
  agentId: string;
  /** The accounts list read for the agent (`GET …/accounts?agent_id=`), or any list that carries the agent's entries. */
  accounts: Account[];
  /** The workspace's Internet access; `null` when the viewer cannot read it. */
  internet: InternetAccess | null;
}

/** "GitHub", "GitHub and Linear", "GitHub, Linear and Notion", "GitHub, Linear and 3 more". */
export function nameList(names: string[]): string {
  if (names.length <= 1) return names.join("");
  if (names.length <= 3) return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return `${names.slice(0, 2).join(", ")} and ${names.length - 2} more`;
}

/** What a row is called in a sentence: the app, else the account (a website). Same names are told apart by the account. */
function labels(accounts: Account[]): Map<Account, string> {
  const base = (a: Account) => a.app?.name ?? a.name;
  const count = new Map<string, number>();
  for (const a of accounts) count.set(base(a), (count.get(base(a)) ?? 0) + 1);
  return new Map(accounts.map((a) => [a, (count.get(base(a)) ?? 0) > 1 && a.name !== base(a) ? `${base(a)} (${a.name})` : base(a)]));
}

const unique = (xs: string[]) => [...new Set(xs)];

/** "creating issues" / "creating issues and closing issues" / "3 kinds of change you always allow". */
function exceptions(names: string[]): string {
  if (names.length <= 2) return names.length === 2 ? `${names[0]} and ${names[1]}` : (names[0] ?? "");
  return `${names.length} kinds of change you always allow`;
}

export function agentSummary({ agent, agentId, accounts, internet }: AgentSummaryInput): string[] {
  const given = accounts.filter((a) => a.given !== false && a.kind !== "ai_key" && a.health.state !== "disabled");
  const name = labels(given);
  const entry = (a: Account) => a.agents.find((e) => e.agent.id === agentId);
  const broken = given.filter(needsAttention);
  const usable = given.filter((a) => !needsAttention(a));

  const level = (a: Account) => entry(a)?.level ?? "use";
  const reads = usable.filter((a) => level(a) !== "custom");
  const asking = usable.filter((a) => level(a) === "ask");
  const acting = usable.filter((a) => level(a) === "act");
  const custom = usable.filter((a) => level(a) === "custom");

  const out: string[] = [];
  // The first sentence names the agent; the rest say "It".
  const subject = () => (out.length === 0 ? agent : "It");

  if (reads.length > 0) out.push(`${agent} can read ${nameList(unique(reads.map((a) => name.get(a)!)))}.`);

  // Ask before changes: one sentence for the accounts with nothing always allowed, one for each with exceptions.
  const plainAsk = asking.filter((a) => (entry(a)?.always_allowed ?? []).length === 0);
  if (plainAsk.length > 0) out.push(`${subject()} asks you before changing anything in ${nameList(unique(plainAsk.map((a) => name.get(a)!)))}.`);
  for (const a of asking) {
    const always = unique((entry(a)?.always_allowed ?? []).map((x) => x.name));
    if (always.length > 0) out.push(`${subject()} asks you before changing anything in ${name.get(a)!}, except ${exceptions(always)}.`);
  }

  if (acting.length > 0) out.push(`${subject()} can change ${nameList(unique(acting.map((a) => name.get(a)!)))} without asking.`);

  for (const a of custom) out.push(`In ${name.get(a)!} ${out.length === 0 ? agent : "it"} has custom access.`);

  for (const a of broken) out.push(`${subject()} can't use ${name.get(a)!} right now: ${healthClause(a)}.`);

  if (internet) {
    const sentence = reachSentence(internet, "It");
    const lead = given.length === 0 ? `${agent} can't use any app yet. ` : "";
    out.push(`${lead}${sentence}`);
    const ask = askSentence(internet);
    // The ask sentence is said only when a rule asks.
    if (ask !== "Nothing asks you first.") out.push(ask);
  } else if (given.length === 0) {
    out.push(`${agent} can't use any app yet.`);
  }
  return out;
}
