import { describe, expect, it } from "vitest";

import type { Account } from "@/lib/api/accounts";
import { agentSummary, nameList } from "@/lib/summary/agent";
import type { InternetAccess } from "@/lib/summary/internet";

// (P5-UX-7, C-986, D-350) Each row is a condition of one agent's accounts and the sentences it must produce, exactly the
// table of `14 §8` in its order. Every row is true under `07 §Evaluation`: `read`, `ask` and `act` (and `use`) all allow
// reads; `ask` asks before every change but the always-allowed ones; `act` changes without asking; an account that needs
// attention cannot be used and is said only as such; an AI key is not an app.

const ME = "a1";
const OPEN: InternetAccess = { withoutAccount: "allow", allowlist: 0, rules: [] };

type Level = "read" | "ask" | "act" | "custom" | "use";
interface Opts {
  kind?: Account["kind"];
  level?: Level;
  always?: string[];
  health?: Account["health"]["state"];
  detail?: string | null;
  /** (C-1033) Why a `failing` account failed. */
  error?: string;
  given?: boolean | null;
  website?: boolean;
}
let n = 0;
function acct(app: string, o: Opts = {}): Account {
  n += 1;
  return {
    ref: { type: "connection", id: `c${n}` },
    kind: o.kind ?? "app",
    name: o.website || o.kind === "website" || o.kind === "ai_key" ? app : `${app} account`,
    app: o.kind === "website" || o.kind === "ai_key" || o.website ? null : { app_id: app.toLowerCase(), name: app, custom_only: false },
    scope: "workspace",
    levels: ["read", "ask", "act"],
    health: { state: o.health ?? "working", detail: o.detail ?? null, last_error: o.error ?? null },
    agents: [
      {
        agent: { id: ME, name: "Daneel", handle: "daneel" },
        workspace: { id: "w1", name: "Main" },
        level: o.level ?? "read",
        always_allowed: (o.always ?? []).map((name, i) => ({ name, tool_id: `t${i}` })),
      },
    ],
    can: { give_access: true, change_access: true, manage: true },
    given: o.given === undefined ? true : o.given,
  } as Account;
}
const sum = (accounts: Account[], internet: InternetAccess | null = OPEN) => agentSummary({ agent: "Daneel", agentId: ME, accounts, internet });

describe("nameList: the accounts it can use", () => {
  it("one, two, three, and more than three", () => {
    expect(nameList(["GitHub"])).toBe("GitHub");
    expect(nameList(["GitHub", "Linear"])).toBe("GitHub and Linear");
    expect(nameList(["GitHub", "Linear", "Notion"])).toBe("GitHub, Linear and Notion");
    expect(nameList(["GitHub", "Linear", "Notion", "AWS", "Slack"])).toBe("GitHub, Linear and 3 more");
  });
});

describe("the agent's summary: a table of conditions", () => {
  const rows: { when: string; accounts: Account[]; internet?: InternetAccess | null; says: string[] }[] = [
    { when: "a new agent: no accounts", accounts: [], says: ["Daneel can't use any app yet. It can reach any website."] },
    {
      when: "only an AI key (not an app, not listed)",
      accounts: [acct("AIGW", { kind: "ai_key", level: "use" })],
      says: ["Daneel can't use any app yet. It can reach any website."],
    },
    {
      when: "read-only apps (and an app with no choice)",
      accounts: [acct("GitHub", { level: "read" }), acct("Linear", { level: "use" })],
      says: ["Daneel can read GitHub and Linear.", "It can reach any website."],
    },
    {
      when: "a website with a token counts among the accounts it can use",
      accounts: [acct("Weather API", { kind: "website", level: "use" }), acct("GitHub", { level: "read" })],
      says: ["Daneel can read Weather API and GitHub.", "It can reach any website."],
    },
    {
      when: "one app asking before changes",
      accounts: [acct("Linear", { level: "ask" })],
      says: ["Daneel can read Linear.", "It asks you before changing anything in Linear.", "It can reach any website."],
    },
    {
      when: "one asking with one always-allowed change",
      accounts: [acct("Linear", { level: "ask", always: ["creating issues"] })],
      says: ["Daneel can read Linear.", "It asks you before changing anything in Linear, except creating issues.", "It can reach any website."],
    },
    {
      when: "one asking with two always-allowed changes",
      accounts: [acct("Linear", { level: "ask", always: ["creating issues", "adding labels"] })],
      says: ["Daneel can read Linear.", "It asks you before changing anything in Linear, except creating issues and adding labels.", "It can reach any website."],
    },
    {
      when: "one asking with three always-allowed changes",
      accounts: [acct("Linear", { level: "ask", always: ["creating issues", "adding labels", "commenting"] })],
      says: ["Daneel can read Linear.", "It asks you before changing anything in Linear, except 3 kinds of change you always allow.", "It can reach any website."],
    },
    {
      when: "two asking apps, one with an exception",
      accounts: [acct("Linear", { level: "ask", always: ["creating issues"] }), acct("Notion", { level: "ask" })],
      says: [
        "Daneel can read Linear and Notion.",
        "It asks you before changing anything in Notion.",
        "It asks you before changing anything in Linear, except creating issues.",
        "It can reach any website.",
      ],
    },
    {
      when: "one changing without asking",
      accounts: [acct("AWS", { level: "act" })],
      says: ["Daneel can read AWS.", "It can change AWS without asking.", "It can reach any website."],
    },
    {
      when: "custom access: said as custom, never as 'can read'",
      accounts: [acct("Notion", { level: "custom" })],
      says: ["In Notion Daneel has custom access.", "It can reach any website."],
    },
    {
      when: "custom beside others",
      accounts: [acct("GitHub", { level: "read" }), acct("Notion", { level: "custom" })],
      says: ["Daneel can read GitHub.", "In Notion it has custom access.", "It can reach any website."],
    },
    {
      when: "a broken account is not usable: its own sentence, and in no other",
      accounts: [acct("GitHub", { level: "read" }), acct("AWS", { level: "act", health: "sign_in_expired" })],
      says: ["Daneel can read GitHub.", "It can't use AWS right now: the sign-in expired.", "It can reach any website."],
    },
    {
      when: "only a broken account: the agent is named, and it is not 'no app yet'",
      accounts: [acct("AWS", { level: "act", health: "sign_in_expired" })],
      says: ["Daneel can't use AWS right now: the sign-in expired.", "It can reach any website."],
    },
    {
      when: "a refused token and an unreachable host",
      accounts: [acct("GitHub", { health: "failing", error: "rejected", detail: "token" }), acct("Postgres", { health: "failing", error: "unreachable", detail: "db.example.org" })],
      says: [
        "Daneel can't use GitHub right now: it no longer accepts this token.",
        "It can't use Postgres right now: it could not be reached (db.example.org).",
        "It can reach any website.",
      ],
    },
    {
      when: "a turned-off account is not an account it can use",
      accounts: [acct("GitHub", { level: "read", health: "disabled" })],
      says: ["Daneel can't use any app yet. It can reach any website."],
    },
    {
      when: "an account not given to the agent (the list read for it also carries the others)",
      accounts: [acct("GitHub", { level: "read", given: false })],
      says: ["Daneel can't use any app yet. It can reach any website."],
    },
    {
      when: "a long list",
      accounts: ["GitHub", "Linear", "Notion", "AWS", "Slack"].map((a) => acct(a, { level: "read" })),
      says: ["Daneel can read GitHub, Linear and 3 more.", "It can reach any website."],
    },
    {
      when: "an account a person named is told by that name, not the app's",
      accounts: [{ ...acct("Calendar link (ICS)", { level: "use" }), name: "Team calendar" }],
      says: ["Daneel can read Team calendar.", "It can reach any website."],
    },
    {
      when: "two accounts of one app are told apart by their names",
      accounts: [{ ...acct("GitHub", { level: "read" }), name: "work" }, { ...acct("GitHub", { level: "read" }), name: "personal" }],
      says: ["Daneel can read GitHub (work) and GitHub (personal).", "It can reach any website."],
    },
    {
      when: "websites are restricted to the allow-list",
      accounts: [acct("GitHub", { level: "read" })],
      internet: { withoutAccount: "block", allowlist: 2, rules: [] },
      says: ["Daneel can read GitHub.", "It can reach only the websites on the allow-list."],
    },
    {
      when: "a rule asks: its sentence follows the reach sentence",
      accounts: [],
      internet: { withoutAccount: "allow", allowlist: 0, rules: [{ enabled: true, action: "allow", requireApproval: true, hosts: ["a.example", "b.example"] }] },
      says: ["Daneel can't use any app yet. It can reach any website.", "A rule asks you first on 2 websites."],
    },
    {
      when: "the viewer cannot read the Internet access: nothing is said about websites",
      accounts: [acct("GitHub", { level: "read" })],
      internet: null,
      says: ["Daneel can read GitHub."],
    },
    { when: "no accounts and the Internet access unknown", accounts: [], internet: null, says: ["Daneel can't use any app yet."] },
  ];
  for (const r of rows) {
    it(r.when, () => {
      expect(sum(r.accounts, r.internet === undefined ? OPEN : r.internet)).toEqual(r.says);
    });
  }
});

describe("Welcome's reach clause (firstday-5)", () => {
  const open = { withoutAccount: "allow" as const, allowlist: 0, rules: [] };
  const base = { agent: "Daneel", agentId: "a1", accounts: [], internet: open };
  it("owners get a way out; members a plain fact; every other page keeps the table of 14 §8", () => {
    expect(agentSummary({ ...base, welcome: { canLimit: true } })).toEqual(["Daneel can't use any app yet. It can reach websites; you can limit that."]);
    expect(agentSummary({ ...base, welcome: { canLimit: false } })).toEqual(["Daneel can't use any app yet. It can reach websites."]);
    expect(agentSummary(base)).toEqual(["Daneel can't use any app yet. It can reach any website."]);
  });
  it("keeps the exact sentence where it says more than 'any website'", () => {
    const blocks = { ...open, rules: [{ enabled: true, action: "block" as const, hosts: ["x.example"], requireApproval: false }] };
    expect(agentSummary({ ...base, internet: blocks as never, welcome: { canLimit: true } })).toEqual([
      "Daneel can't use any app yet. It can reach any website, except where a rule blocks it.",
    ]);
  });
});
