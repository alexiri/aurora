import { describe, expect, it } from "vitest";

import { askSentence, internetAccessOf, internetSummary, reachSentence, type InternetAccess, type InternetRule } from "@/lib/summary/internet";

// (P5-IA-2, C-986) Each row is a condition of a level's Internet access and the sentences it must produce. Every row is
// checked against `07 §Evaluation` (rules first; then, for a request without an account, `block` allows only the
// allow-list; the default rule only ever blocks a request that carries an account's credential):
//   - `withoutAccount: "allow"`, no blocking rule     -> a website is reachable.
//   - `withoutAccount: "allow"`, a blocking rule      -> reachable except where the rule matches.
//   - `withoutAccount: "block"`                       -> only the allow-list, and what an enabled allow rule matches.
//   - a rule with no targets matches every request.
const allow = (hosts: string[] | null, over: Partial<InternetRule> = {}): InternetRule => ({ enabled: true, action: "allow", requireApproval: false, hosts, ...over });
const block = (hosts: string[] | null, over: Partial<InternetRule> = {}): InternetRule => ({ enabled: true, action: "block", requireApproval: false, hosts, ...over });
const ask = (hosts: string[] | null, over: Partial<InternetRule> = {}): InternetRule => allow(hosts, { requireApproval: true, ...over });

interface Row {
  when: string;
  access: InternetAccess;
  reach: string;
  asks: string;
}

const ROWS: Row[] = [
  {
    when: "a fresh workspace: open, no rules (07 §Evaluation: a request without an account is allowed, a request no rule matches is allowed)",
    access: { withoutAccount: "allow", allowlist: 0, rules: [] },
    reach: "Agents here can reach any website.",
    asks: "Nothing asks you first.",
  },
  {
    when: "open, an allow-list that is not used (it only applies under block)",
    access: { withoutAccount: "allow", allowlist: 3, rules: [] },
    reach: "Agents here can reach any website.",
    asks: "Nothing asks you first.",
  },
  {
    when: "requests without an account are blocked and the allow-list names websites",
    access: { withoutAccount: "block", allowlist: 2, rules: [] },
    reach: "Agents here can reach only the websites on the allow-list.",
    asks: "Nothing asks you first.",
  },
  {
    when: "blocked, an empty allow-list, no rule opens anything",
    access: { withoutAccount: "block", allowlist: 0, rules: [] },
    reach: "Agents here can't reach the internet.",
    asks: "Nothing asks you first.",
  },
  {
    when: "blocked, an empty allow-list, but an enabled allow rule names a host (rules are evaluated before defaults)",
    access: { withoutAccount: "block", allowlist: 0, rules: [allow(["api.example.org"])] },
    reach: "Agents here can reach only the websites on the allow-list and those a rule allows.",
    asks: "Nothing asks you first.",
  },
  {
    when: "blocked, an allow-list and an allow rule",
    access: { withoutAccount: "block", allowlist: 1, rules: [allow(["api.example.org"])] },
    reach: "Agents here can reach only the websites on the allow-list and those a rule allows.",
    asks: "Nothing asks you first.",
  },
  {
    when: "blocked, but the only allow rule is turned off",
    access: { withoutAccount: "block", allowlist: 1, rules: [allow(["api.example.org"], { enabled: false })] },
    reach: "Agents here can reach only the websites on the allow-list.",
    asks: "Nothing asks you first.",
  },
  {
    when: "an allow rule with no targets matches every request, even under block",
    access: { withoutAccount: "block", allowlist: 0, rules: [allow([])] },
    reach: "Agents here can reach any website.",
    asks: "Nothing asks you first.",
  },
  {
    when: "open, an enabled rule blocks a host",
    access: { withoutAccount: "allow", allowlist: 0, rules: [block(["evil.example.org"])] },
    reach: "Agents here can reach any website, except where a rule blocks it.",
    asks: "Nothing asks you first.",
  },
  {
    when: "open, the blocking rule is turned off",
    access: { withoutAccount: "allow", allowlist: 0, rules: [block(["evil.example.org"], { enabled: false })] },
    reach: "Agents here can reach any website.",
    asks: "Nothing asks you first.",
  },
  {
    when: "a block rule with no targets matches every request",
    access: { withoutAccount: "allow", allowlist: 0, rules: [block([])] },
    reach: "Agents here can't reach the internet.",
    asks: "Nothing asks you first.",
  },
  {
    when: "a block rule whose targets are not websites (a secret, an app) says nothing about reach",
    access: { withoutAccount: "allow", allowlist: 0, rules: [block(null)] },
    reach: "Agents here can reach any website.",
    asks: "Nothing asks you first.",
  },
  {
    when: "open, one rule asks on one website",
    access: { withoutAccount: "allow", allowlist: 0, rules: [ask(["github.com"])] },
    reach: "Agents here can reach any website.",
    asks: "A rule asks you first on 1 website.",
  },
  {
    when: "two asking rules naming three distinct hosts (a host named twice counts once)",
    access: { withoutAccount: "allow", allowlist: 0, rules: [ask(["a.example.org", "b.example.org"]), ask(["b.example.org", "c.example.org"])] },
    reach: "Agents here can reach any website.",
    asks: "A rule asks you first on 3 websites.",
  },
  {
    when: "an asking rule that is turned off asks nothing",
    access: { withoutAccount: "allow", allowlist: 0, rules: [ask(["github.com"], { enabled: false })] },
    reach: "Agents here can reach any website.",
    asks: "Nothing asks you first.",
  },
  {
    when: "an asking rule with no targets asks on every request",
    access: { withoutAccount: "allow", allowlist: 0, rules: [ask([])] },
    reach: "Agents here can reach any website.",
    asks: "A rule asks you first on every website.",
  },
  {
    when: "an asking rule about a secret or an app, not a website",
    access: { withoutAccount: "allow", allowlist: 0, rules: [ask(null)] },
    reach: "Agents here can reach any website.",
    asks: "A rule asks you first.",
  },
  {
    when: "a block rule never counts as asking, whatever its flag says",
    access: { withoutAccount: "allow", allowlist: 0, rules: [block(["x.example.org"], { requireApproval: true })] },
    reach: "Agents here can reach any website, except where a rule blocks it.",
    asks: "Nothing asks you first.",
  },
  {
    when: "blocked with the allow-list and a rule that asks about an allowed host",
    access: { withoutAccount: "block", allowlist: 1, rules: [ask(["api.example.org"])] },
    reach: "Agents here can reach only the websites on the allow-list and those a rule allows.",
    asks: "A rule asks you first on 1 website.",
  },
];

describe("Internet access summary (C-986)", () => {
  it.each(ROWS)("$when", ({ access, reach, asks }) => {
    expect(reachSentence(access, "Agents here")).toBe(reach);
    expect(askSentence(access)).toBe(asks);
    expect(internetSummary(access, "Agents here")).toBe(`${reach} ${asks}`);
  });

  it("opens a fresh workspace's page with the sentence the spec quotes", () => {
    expect(internetSummary({ withoutAccount: "allow", allowlist: 0, rules: [] }, "Agents here")).toBe("Agents here can reach any website. Nothing asks you first.");
  });

  it("takes the subject as a parameter", () => {
    expect(reachSentence({ withoutAccount: "allow", allowlist: 0, rules: [] }, "It")).toBe("It can reach any website.");
    expect(reachSentence({ withoutAccount: "block", allowlist: 0, rules: [] }, "Daneel")).toBe("Daneel can't reach the internet.");
  });

  it("reads a policy generation: a level above that blocks requests without an account tightens every level below", () => {
    const gen = { rules: [], uncredentialed_default: "none" as const, uncredentialed_allowlist: [{ host: "pypi.org" }] };
    expect(internetAccessOf(gen).withoutAccount).toBe("allow");
    expect(internetAccessOf(gen, true)).toMatchObject({ withoutAccount: "block", allowlist: 1 });
    expect(internetAccessOf({ ...gen, uncredentialed_default: "block" }).withoutAccount).toBe("block");
  });

  it("reads a rule's targets: none matches everything, other kinds say nothing about websites", () => {
    const base = { name: "r", enabled: true, action: "allow" as const, require_approval: true, identities: [], conditions: [] };
    const gen = (targets: unknown[]) => ({
      rules: [{ ...base, targets }] as never,
      uncredentialed_default: "none" as const,
      uncredentialed_allowlist: [],
    });
    expect(internetAccessOf(gen([])).rules[0]!.hosts).toEqual([]);
    expect(internetAccessOf(gen([{ type: "secret", id: "s1" }])).rules[0]!.hosts).toBeNull();
    expect(internetAccessOf(gen([{ type: "network", host: "github.com" }, { type: "secret", id: "s1" }])).rules[0]!.hosts).toEqual(["github.com"]);
  });
});
