import type { PolicyGeneration } from "@/lib/api/policy";

/**
 * (P5-IA-2, C-986) What a level's Internet access says in words: a **reach** sentence and an **ask** sentence. Pure; the
 * unit tests walk the table of conditions below, and every sentence must be true under `07 §Evaluation`:
 *
 * ```
 * verdict = first enabled rule that matches (block | allow | allow + approval)
 *           else, for a request without an account: block unless the allow-list names it, when the default is `block`
 *           else the default rule (which only ever blocks a request that carries an account's credential)
 * ```
 *
 * So, for a request **without an account** (the websites these sentences are about; the accounts an agent has are
 * said beside them, `14 §8`): with `allow` it reaches any website except where an enabled rule blocks; with `block`
 * only the allow-list (and what an enabled allow rule matches) is reachable. Rules are evaluated before defaults, so an
 * allow rule still opens a site under `block`, and a rule that needs approval makes the request wait for a person.
 */

/** One enabled-or-not rule, reduced to what the sentences need. */
export interface InternetRule {
  enabled: boolean;
  action: "allow" | "block";
  requireApproval: boolean;
  /**
   * The hosts the rule's network targets name (patterns count once each). `[]`: the rule has no targets, so it matches
   * every request. `null`: it has targets of another kind only (a secret, an app), which says nothing about websites.
   */
  hosts: string[] | null;
}

export interface InternetAccess {
  /** What happens to a request without an account: `allow`, or `block` unless the allow-list names the website. */
  withoutAccount: "allow" | "block";
  /** Entries in the allow-list for requests without an account. */
  allowlist: number;
  rules: InternetRule[];
}

const enabled = (r: InternetRule) => r.enabled;
const hostsOf = (rules: InternetRule[]): Set<string> => new Set(rules.flatMap((r) => r.hosts ?? []));

/** "<Subject> can reach any website." and its siblings. */
export function reachSentence(a: InternetAccess, subject: string): string {
  const live = a.rules.filter(enabled);
  // A rule with no target blocks (or allows) every request: nothing else about the level can be true.
  const blocksAll = live.some((r) => r.action === "block" && r.hosts?.length === 0);
  const blocksSome = live.some((r) => r.action === "block" && (r.hosts?.length ?? 0) > 0);
  const allowsSome = live.some((r) => r.action === "allow" && (r.hosts?.length ?? 0) > 0);
  const allowsAll = live.some((r) => r.action === "allow" && r.hosts?.length === 0);
  if (blocksAll) return `${subject} can't reach the internet.`;
  if (a.withoutAccount === "allow" || allowsAll) {
    return blocksSome ? `${subject} can reach any website, except where a rule blocks it.` : `${subject} can reach any website.`;
  }
  if (a.allowlist === 0 && !allowsSome) return `${subject} can't reach the internet.`;
  return allowsSome
    ? `${subject} can reach only the websites on the allow-list and those a rule allows.`
    : `${subject} can reach only the websites on the allow-list.`;
}

/** "Nothing asks you first." / "A rule asks you first on <n> websites." */
export function askSentence(a: InternetAccess): string {
  const asking = a.rules.filter((r) => r.enabled && r.action === "allow" && r.requireApproval);
  if (asking.length === 0) return "Nothing asks you first.";
  if (asking.some((r) => r.hosts?.length === 0)) return "A rule asks you first on every website.";
  const n = hostsOf(asking).size;
  // Targets of another kind only (a secret, an app): a rule asks, but not about a website.
  if (n === 0) return "A rule asks you first.";
  return `A rule asks you first on ${n} ${n === 1 ? "website" : "websites"}.`;
}

/** The two sentences of an Internet access page, joined: "Agents here can reach any website. Nothing asks you first." */
export function internetSummary(a: InternetAccess, subject: string): string {
  return `${reachSentence(a, subject)} ${askSentence(a)}`;
}

/**
 * (FIX-206 follow-up, D-444) What an organization member reads in force, in one sentence, from the effective governance
 * (`GET /v1/orgs/{org}/governance/effective`, readable by any organization member): the posture for requests without an
 * account and the allow-lists. The organization's rules are not a member's read, so the sentence leaves room for them, and
 * for workspaces, which can only narrow it.
 */
export function orgMemberSummary(egress: { posture: { value: string }; baseline_allowlist: unknown[]; org_allowlist: unknown[] }, orgName: string): string {
  if (egress.posture.value !== "credentialed_only") {
    return `In ${orgName}, requests without an account can reach any website unless a rule or a workspace blocks them.`;
  }
  const n = egress.baseline_allowlist.length + egress.org_allowlist.length;
  if (n === 0) return `In ${orgName}, requests without an account reach only the websites a rule allows.`;
  return `In ${orgName}, requests without an account reach only the ${n} ${n === 1 ? "website" : "websites"} on the allow-list and those a rule allows.`;
}

/**
 * A level's `InternetAccess` from its policy generation, given whether a level above already blocks requests without an
 * account (the instance's or the organization's `credentialed_only` posture, which a level below can only tighten).
 */
export function internetAccessOf(
  gen: Pick<PolicyGeneration, "rules" | "uncredentialed_default" | "uncredentialed_allowlist">,
  aboveBlocks = false,
  /** Entries of the allow-lists above (the instance's baseline and the organization's): they are on the list here too. */
  aboveAllowlist = 0,
): InternetAccess {
  return {
    withoutAccount: aboveBlocks || gen.uncredentialed_default === "block" ? "block" : "allow",
    allowlist: gen.uncredentialed_allowlist.length + aboveAllowlist,
    rules: gen.rules.map((r) => {
      const targets = r.targets ?? [];
      const network = targets.filter((t) => t.type === "network");
      return {
        enabled: r.enabled,
        action: r.action === "block" ? "block" : "allow",
        requireApproval: Boolean(r.require_approval),
        hosts: targets.length === 0 ? [] : network.length === 0 ? null : network.map((t) => (t.type === "network" ? t.host : "")),
      };
    }),
  };
}
