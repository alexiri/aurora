import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { InternetLevel } from "@/components/internet/internet-level";
import { OrgInternet } from "@/components/internet/org-internet";
import type { DraftState, PolicyGeneration } from "@/lib/api/policy";
import { commonChangeRule, hostOf, limitsFromAbove, pendingChanges } from "@/lib/internet-access";
import { wrap } from "./support/render";
import { type Handler, json, mockFetch } from "./support/api";

const gen = (over: Partial<PolicyGeneration> = {}): PolicyGeneration => ({
  id: "g1", number: 1, status: "draft", default_action: "none", uncredentialed_default: "none",
  uncredentialed_allowlist: [], open_lane_posture: "enabled", rules: [], draft_revision: 3, created_at: "2026-01-01T00:00:00Z", ...over,
});
const EFF = {
  llm: { posture: { value: "open", source: "org" }, endpoints: [] },
  egress: { posture: { value: "open", source: "org" }, open_lane: { value: "enabled", source: "org" }, org_allowlist: [], baseline_allowlist: [] },
  byo: { posture: { value: "owners_only", source: "instance" }, source_cidrs: { value: null, source: "instance" }, require_context_credentials: { value: false, source: "instance" } },
  agent_tokens: { max_age_days: { value: null, source: "instance" } },
  non_compliant_secrets: [],
};
/** A policy of one scope that behaves like the server: a draft that keeps what was written, a publish that copies it. */
function policyServer(base: string, kind: "workspace" | "organization", opts: { publishFails?: boolean } = {}) {
  let draft: Record<string, unknown>[] = [];
  let published: Record<string, unknown>[] = [];
  let rev = 3;
  const puts: { revision: number; rules: Record<string, unknown>[] }[] = [];
  const publishes: { revision: number }[] = [];
  const g = (rules: Record<string, unknown>[], status: "draft" | "published", revision: number | null) =>
    gen({
      status,
      draft_revision: revision,
      rules: rules.map((x, i) => ({ priority: i, description: null, rate_limit: null, conditions: [], identities: [], enabled: true, require_approval: false, ...x, logical_id: (x.logical_id as string) ?? `L-${String(x.name)}` })) as never,
    });
  const handlers: Record<string, Handler> = {
    [`GET ${base}`]: () => json(200, { scope: kind, published: g(published, "published", null), draft: g(draft, "draft", rev) }),
    [`GET ${base}/warnings`]: () => json(200, { items: [] }),
    [`PUT ${base}/draft`]: async (req) => {
      const b = await req.json();
      puts.push(b);
      draft = b.rules.map((r: Record<string, unknown>) => ({ ...r, logical_id: r.logical_id ?? `L-${String(r.name)}` }));
      rev += 1;
      return json(200, g(draft, "draft", rev));
    },
    [`POST ${base}/publish`]: async (req) => {
      publishes.push(await req.json());
      if (opts.publishFails) return json(500, { error: { code: "internal", message: "boom" } });
      published = draft;
      return json(200, { scope: kind, published: g(published, "published", null), draft: g(draft, "draft", rev) });
    },
  };
  return { handlers, puts, publishes };
}

const WHO = { role: "workspace_owner", action: "change_this", people: [], can: false } as never;
const state = (over: Partial<DraftState> = {}): DraftState => ({ default_action: "none", uncredentialed_default: "none", uncredentialed_allowlist: [], open_lane_posture: "enabled", rules: [], ...over });

describe("Common changes and pending changes (C-989)", () => {
  it("reads the host of what a person typed", () => {
    expect(hostOf("https://Status.Example.org/path?q=1")).toBe("status.example.org");
    expect(hostOf("  *.example.org ")).toBe("*.example.org");
    expect(hostOf("api.example.org:8443")).toBe("api.example.org");
    expect(hostOf("")).toBeNull();
    expect(hostOf("not a host")).toBeNull();
  });

  it("each Common change is an ordinary rule", () => {
    expect(commonChangeRule("ask_before_changes", "status.example.org")).toMatchObject({
      name: "Ask before changes on status.example.org",
      action: "allow",
      require_approval: true,
      enabled: true,
      identities: [],
      targets: [{ type: "network", host: "status.example.org", methods: ["POST", "PUT", "PATCH", "DELETE"] }],
    });
    expect(commonChangeRule("block", "pastebin.com")).toMatchObject({
      name: "Block pastebin.com",
      action: "block",
      require_approval: false,
      targets: [{ type: "network", host: "pastebin.com" }],
    });
  });

  it("counts what the draft holds that is not applied", () => {
    const r = (id: string, name = id) => ({ ...commonChangeRule("block", `${id}.io`), logical_id: id, name });
    const published = state({ rules: [r("a"), r("b")] });
    expect(pendingChanges(published, published)).toBe(0);
    expect(pendingChanges(published, state({ rules: [r("a"), r("b"), commonChangeRule("block", "new.io")] }))).toBe(1);
    expect(pendingChanges(published, state({ rules: [r("a")] }))).toBe(1);
    expect(pendingChanges(published, state({ rules: [r("a", "renamed"), r("b")] }))).toBe(1);
    expect(pendingChanges(published, state({ rules: [r("b"), r("a")] }))).toBe(1);
    expect(pendingChanges(published, state({ rules: published.rules, default_action: "block", open_lane_posture: "disabled" }))).toBe(2);
    expect(pendingChanges(state(), state({ uncredentialed_allowlist: [{ host: "docs.example.org", path: null }] }))).toBe(1);
    // A blank allow-list row is not a change.
    expect(pendingChanges(state(), state({ uncredentialed_allowlist: [{ host: " ", path: null }] }))).toBe(0);
  });

  it("puts the limits from above in words, with who set them", () => {
    const eff = {
      ...EFF,
      egress: {
        posture: { value: "credentialed_only", source: "instance" },
        open_lane: { value: "enabled", source: "org" },
        org_allowlist: [{ host: "pypi.org" }],
        baseline_allowlist: [{ host: "a.io" }, { host: "b.io" }, { host: "c.io" }],
      },
    };
    const who = { org: WHO, instance: WHO };
    const ws = limitsFromAbove("workspace", eff as never, who);
    expect(ws.map((r) => [r.label, r.value_display, r.source])).toEqual([
      ["Websites agents may reach", "Only listed websites", "instance"],
      ["Browser connections (not inspected)", "Allowed", "org"],
      ["Allowed everywhere", "a.io, b.io and 1 more", "instance"],
      ["Allowed in this organization", "pypi.org", "org"],
    ]);
    // The organization sees only what Administration sets: its own values are its controls.
    expect(limitsFromAbove("org", eff as never, who).map((r) => r.label)).toEqual(["Websites agents may reach", "Allowed everywhere"]);
  });
});

describe("a workspace's Internet access page", () => {
  const scope = { kind: "workspace", id: "w1" } as const;
  let srv = policyServer("/v1/workspaces/w1/policy", "workspace");
  function routes() {
    srv = policyServer("/v1/workspaces/w1/policy", "workspace");
    return mockFetch({
      ...srv.handlers,
      "GET /v1/workspaces/w1/secrets": () => json(200, { items: [] }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [] }),
    });
  }
  const page = () => wrap(<InternetLevel level="workspace" scope={scope} orgId="o1" orgSlug="acme" wsSlug="main" canEdit eff={EFF as never} whoCanHere={WHO} whoCanOrg={WHO} />);

  it("opens on the summary, the line about apps, Common changes and the empty rules; the controls are closed", async () => {
    routes();
    page();
    expect((await screen.findByTestId("internet-summary")).textContent).toBe("Agents here can reach any website. Nothing asks you first.");
    expect(screen.getByRole("link", { name: "Apps & access tab" })).toHaveAttribute("href", "/o/acme/w/main/agents");
    expect(screen.getByRole("button", { name: /Ask before changes on a website/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Block a website/ })).toBeInTheDocument();
    expect(screen.getByText("No rules yet. Until you add one, requests follow the limits and defaults below.")).toBeInTheDocument();
    expect(screen.queryByLabelText("Requests without an account")).toBeNull();
    expect(screen.getByRole("button", { name: "Restrict further…" })).toHaveAttribute("aria-expanded", "false");
    // (P5-POLISH-7) The simulator is a button that opens it in a dialog.
    expect(screen.queryByRole("heading", { name: "Try a request", level: 2 })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Try a request" }));
    expect(await screen.findByRole("dialog", { name: "Try a request" })).toBeInTheDocument();
    fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" });
    // Nothing is pending: no bar.
    expect(screen.queryByRole("region", { name: "Changes not applied" })).toBeNull();
  });

  it("FIX-164: a member reads the summary and the published rules as text, with no controls, from the member route only", async () => {
    const fn = mockFetch({
      "GET /v1/workspaces/w1/internet-access": () =>
        json(200, {
          number: 2, default_action: "none", uncredentialed_default: "none", uncredentialed_allowlist: [], open_lane_posture: "enabled",
          rules: [
            { logical_id: "r1", priority: 0, name: "No evil", description: "Never", enabled: true, identities: [], targets: [{ type: "network", host: "evil.example.com" }], action: "block", require_approval: false, conditions: [] },
            { logical_id: "r2", priority: 1, name: "Ask for the bank", enabled: true, identities: [], targets: [{ type: "network", host: "bank.example.com" }], action: "allow", require_approval: true, conditions: [] },
          ],
        }),
    });
    wrap(<InternetLevel level="workspace" scope={scope} orgId="o1" orgSlug="acme" wsSlug="main" canEdit={false} eff={EFF as never} whoCanHere={WHO} whoCanOrg={WHO} />);
    expect((await screen.findByTestId("internet-summary")).textContent).toBe("Agents here can reach any website, except where a rule blocks it. A rule asks you first on 1 website.");
    const rules = await screen.findByRole("list", { name: "Your rules" });
    expect(within(rules).getByText("No evil")).toBeInTheDocument();
    expect(within(rules).getByText("Blocks")).toBeInTheDocument();
    expect(within(rules).getByText("Asks")).toBeInTheDocument();
    expect(within(rules).getByText("evil.example.com")).toBeInTheDocument();
    expect(screen.getByText("Only workspace owners can change this.")).toBeInTheDocument();
    // Read-only: no Common changes, Add rule, Restrict further…, Edit/Delete.
    for (const name of [/Block a website/, /Ask before changes on a website/, /Add rule/, /Restrict further/, /^Edit /, /^Delete /]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
    // Only the member route was read: never the owner's policy document, its warnings or history.
    const paths = fn.mock.calls.map((c) => new URL((c[0] as Request).url, "http://localhost").pathname);
    expect(paths.some((p) => p === "/v1/workspaces/w1/policy" || p.endsWith("/policy/warnings") || p.endsWith("/policy/generations"))).toBe(false);
  });

  it("Block a website shows the rule, adds it to the draft, and Apply publishes it", async () => {
    routes();
    const { puts, publishes } = srv;
    page();
    fireEvent.click(await screen.findByRole("button", { name: /Block a website/ }));
    fireEvent.change(await screen.findByLabelText("Website"), { target: { value: "https://Pastebin.com/raw" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the rule" }));
    const preview = await screen.findByTestId("common-change-rule");
    expect(preview.textContent).toContain("Block pastebin.com");
    expect(puts).toHaveLength(0);
    fireEvent.click(screen.getByRole("button", { name: "Add rule" }));
    // Written to the draft as it is made, and shown as not applied.
    await waitFor(() => expect(puts).toHaveLength(1));
    expect(puts[0].revision).toBe(3);
    expect(puts[0].rules).toMatchObject([{ name: "Block pastebin.com", action: "block", targets: [{ host: "pastebin.com" }] }]);
    expect(await screen.findByText("Not applied yet")).toBeInTheDocument();
    const bar = screen.getByRole("region", { name: "Changes not applied" });
    expect(within(bar).getByText("1 change not applied yet")).toBeInTheDocument();
    expect(publishes).toHaveLength(0);
    fireEvent.click(within(bar).getByRole("button", { name: "Apply changes" }));
    await waitFor(() => expect(publishes).toEqual([{ revision: 4 }]));
    // Applied: the rule is no longer marked and the bar is gone.
    await waitFor(() => expect(screen.queryByText("Not applied yet")).toBeNull());
    expect(screen.queryByRole("region", { name: "Changes not applied" })).toBeNull();
  });

  it("Ask before changes on a website adds an allow rule that needs approval on the changing methods", async () => {
    routes();
    page();
    fireEvent.click(await screen.findByRole("button", { name: /Ask before changes on a website/ }));
    fireEvent.change(await screen.findByLabelText("Website"), { target: { value: "status.example.org" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the rule" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add rule" }));
    await waitFor(() => expect(srv.puts).toHaveLength(1));
    const rule = srv.puts[0].rules[0] as { name: string; action: string; require_approval: boolean; targets: { methods: string[] }[] };
    expect(rule).toMatchObject({ name: "Ask before changes on status.example.org", action: "allow", require_approval: true });
    expect(rule.targets[0].methods).toEqual(["POST", "PUT", "PATCH", "DELETE"]);
  });

  it("asks for a website before it shows a rule", async () => {
    routes();
    page();
    fireEvent.click(await screen.findByRole("button", { name: /Block a website/ }));
    fireEvent.click(screen.getByRole("button", { name: "Show the rule" }));
    expect(await screen.findByText("Enter a website, like status.example.org.")).toBeInTheDocument();
    expect(screen.queryByTestId("common-change-rule")).toBeNull();
  });

  it("Discard writes the published generation back", async () => {
    routes();
    const { puts } = srv;
    page();
    fireEvent.click(await screen.findByRole("button", { name: /Block a website/ }));
    fireEvent.change(await screen.findByLabelText("Website"), { target: { value: "pastebin.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the rule" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add rule" }));
    const bar = await screen.findByRole("region", { name: "Changes not applied" });
    fireEvent.click(within(bar).getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(puts).toHaveLength(2));
    expect(puts[1].rules).toEqual([]);
    await waitFor(() => expect(screen.queryByRole("region", { name: "Changes not applied" })).toBeNull());
  });
});

describe("an organization's Internet access page (one bar, two steps)", () => {
  const org = { id: "o1", slug: "acme", name: "Acme", my_role: "owner", contacts: null } as never;
  const doc = { byo: { posture: null, source_cidrs: [], require_context_credentials: false }, agent_tokens: { max_age_days: null }, llm: { posture: "open", endpoints: {} }, egress: { posture: "open", open_lane: "enabled", allowlist: [] } };
  let srv = policyServer("/v1/orgs/o1/policy", "organization");
  function routes(extra: Record<string, Handler> = {}, publishFails = false) {
    srv = policyServer("/v1/orgs/o1/policy", "organization", { publishFails });
    return mockFetch({
      ...srv.handlers,
      "GET /v1/orgs/o1":() => json(200, { id: "o1", slug: "acme", name: "Acme", my_role: "owner" }),
      "GET /v1/orgs/o1/governance/effective": () => json(200, EFF),
      "GET /v1/orgs/o1/governance": () => json(200, { document: doc, revision: 4 }),
      "GET /v1/governance/instance": () => json(200, { llm: { allowed_key_scopes: [], minimum_posture: "open", require_key_origin: false, known_llm_hosts: [], endpoints: [] }, egress: { minimum_posture: "open", open_lane_minimum: "enabled", baseline_allowlist: [] }, byo: { enabled: true, minimum_posture: "owners_only", default_source_cidrs: [], require_context_credentials: false } }),
      "GET /v1/orgs/o1/workspaces": () => json(200, { items: [{ id: "w1", slug: "release", name: "Release" }], next_cursor: null }),
      "GET /v1/orgs/o1/workspace-overrides": () =>
        json(200, { items: [{ workspace: { id: "w1", name: "Release" }, uncredentialed: "block", open_lane: "disabled", default_rule: "none", rule_count: 3 }], next_cursor: null }),
      "GET /v1/orgs/o1/members": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/orgs/o1/groups": () => json(200, { items: [], next_cursor: null }),
      "GET /v1/orgs/o1/secrets": () => json(200, { items: [] }),
      "GET /v1/workspaces/w1/activity": () => json(200, { items: [], next_cursor: null }),
      ...extra,
    });
  }

  it("lists the workspaces that tightened, with a link to each", async () => {
    routes();
    wrap(<OrgInternet org={org} slug="acme" />);
    const row = (await screen.findAllByText("Release"))[0].closest("tr") as HTMLElement;
    expect(within(row).getByText("Blocked unless listed")).toBeInTheDocument();
    expect(within(row).getByText("Turned off")).toBeInTheDocument();
    expect(within(row).getByText("3")).toBeInTheDocument();
    expect(within(row).getByRole("link", { name: "Open" })).toHaveAttribute("href", "/o/acme/w/release/internet-access");
    expect(screen.getByRole("button", { name: "Show all workspaces" })).toBeInTheDocument();
  });

  it("applies the limits first, then the rules; when the rules fail it says which part was applied and keeps the rest pending", async () => {
    const order: string[] = [];
    let govBody: { revision: number; document: { egress: { posture: string } } } | null = null;
    routes(
      {
        "PUT /v1/orgs/o1/governance": async (req) => {
          order.push("governance");
          govBody = await req.json();
          return json(200, { document: govBody!.document, revision: 5 });
        },
      },
      true,
    );
    wrap(<OrgInternet org={org} slug="acme" />);
    fireEvent.change(await screen.findByLabelText("Which websites agents may reach"), { target: { value: "credentialed_only" } });
    fireEvent.click(screen.getByRole("button", { name: /Block a website/ }));
    fireEvent.change(await screen.findByLabelText("Website"), { target: { value: "pastebin.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the rule" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add rule" }));
    const bar = await screen.findByRole("region", { name: "Changes not applied" });
    await waitFor(() => expect(within(bar).getByText("2 changes not applied yet")).toBeInTheDocument());
    fireEvent.click(within(bar).getByRole("button", { name: "Apply changes" }));
    await waitFor(() => expect(srv.publishes).toHaveLength(1));
    expect(order).toEqual(["governance"]);
    expect(govBody!.revision).toBe(4);
    expect(govBody!.document.egress.posture).toBe("credentialed_only");
    // The limits are applied, the rule is not: the bar says so and counts only what is left.
    expect(await within(bar).findByText(/The limits were applied\. The rules and defaults were not/)).toBeInTheDocument();
    await waitFor(() => expect(within(bar).getByText(/^1 change not applied yet/)).toBeInTheDocument());
    // A retry sends only the rules: the document is not sent again.
    fireEvent.click(within(bar).getByRole("button", { name: "Apply changes" }));
    await waitFor(() => expect(srv.publishes).toHaveLength(2));
    expect(order).toEqual(["governance"]);
  });

  it("does not publish the rules when the limits are refused", async () => {
    const order: string[] = [];
    routes({
      "PUT /v1/orgs/o1/governance": () => {
        order.push("governance");
        return json(422, { error: { code: "governance_below_instance_minimum", message: "Below the minimum" } });
      },
    });
    wrap(<OrgInternet org={org} slug="acme" />);
    fireEvent.change(await screen.findByLabelText("Which websites agents may reach"), { target: { value: "credentialed_only" } });
    fireEvent.click(screen.getByRole("button", { name: /Block a website/ }));
    fireEvent.change(await screen.findByLabelText("Website"), { target: { value: "pastebin.com" } });
    fireEvent.click(screen.getByRole("button", { name: "Show the rule" }));
    fireEvent.click(await screen.findByRole("button", { name: "Add rule" }));
    const bar = await screen.findByRole("region", { name: "Changes not applied" });
    fireEvent.click(within(bar).getByRole("button", { name: "Apply changes" }));
    await waitFor(() => expect(order).toEqual(["governance"]));
    expect(await screen.findByText(/Below the minimum/)).toBeInTheDocument();
    expect(order).toEqual(["governance"]);
    expect(srv.publishes).toHaveLength(0);
  });
});
