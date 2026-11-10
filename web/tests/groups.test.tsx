import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { GroupDetail } from "@/components/groups/group-detail";
import { GroupsView } from "@/components/groups/groups-view";
import { RuleEditorDialog } from "@/components/policy/rule-editor";
import { AccessTab } from "@/components/settings/access-tab";

vi.mock("next/navigation", () => ({ useRouter: () => ({ push: vi.fn(), replace: vi.fn() }) }));

const json = (status: number, body: unknown) =>
  new Response(body === null ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });

const ORG = "00000000-0000-4000-8000-000000000001";
const WS = "00000000-0000-4000-8000-000000000002";
const G1 = "00000000-0000-4000-8000-000000000401";
const G2 = "00000000-0000-4000-8000-000000000402";
const U = "00000000-0000-4000-8000-000000000099";
const now = "2026-11-10T10:00:00Z";

const group = (id: string, source: "manual" | "scim" | "idp", extra: Record<string, unknown> = {}) => ({
  id,
  org_id: ORG,
  name: source === "idp" ? "CERN e-group" : "Platform team",
  description: null,
  source,
  idp_provider: source === "idp" ? "corp" : null,
  idp_value: source === "idp" ? "aurora-platform" : null,
  org_role: null,
  member_count: 1,
  workspace_count: 1,
  revision: 1,
  created_at: now,
  ...extra,
});

const member: { user: { id: string; email: string; display_name: string }; status: string; source: string; synced_at: string | null; added_at: string } = {
  user: { id: U, email: "ada@example.test", display_name: "Ada" },
  status: "active",
  source: "manual",
  synced_at: null,
  added_at: now,
};

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={qc}>{ui}</QueryClientProvider>);
}

/** Routes by `METHOD path`; records every call as `METHOD path?query body`. */
function stub(routes: Record<string, (body: unknown) => Response>) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const body = input.method === "GET" ? undefined : await input.json().catch(() => undefined);
      calls.push(`${input.method} ${url.pathname}${url.search} ${body === undefined ? "" : JSON.stringify(body)}`.trim());
      const h = routes[`${input.method} ${url.pathname}`];
      return h ? h(body) : json(404, { error: { code: "not_found", message: "nf" } });
    }),
  );
  return calls;
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("Groups list", () => {
  it("shows name, source badge, member count and org role", async () => {
    stub({
      [`GET /v1/orgs/${ORG}/groups`]: () =>
        json(200, { items: [group(G1, "manual"), group(G2, "idp", { org_role: "admin" })], next_cursor: null }),
      "GET /v1/instance": () => json(200, { oidc_providers: [{ id: "corp", display_name: "Corp", groups: true }] }),
    });
    wrap(<GroupsView orgId={ORG} orgSlug="acme" />);
    expect(await screen.findByRole("link", { name: "Platform team" })).toHaveAttribute("href", `/o/acme/groups/${G1}`);
    expect(screen.getByText("Manual")).toBeInTheDocument();
    expect(screen.getByText("Sign-in")).toBeInTheDocument();
    expect(screen.getByText("admin")).toBeInTheDocument();
  });

  it("offers the sign-in link only for providers with a group claim", async () => {
    stub({
      [`GET /v1/orgs/${ORG}/groups`]: () => json(200, { items: [], next_cursor: null }),
      "GET /v1/instance": () =>
        json(200, { oidc_providers: [{ id: "corp", display_name: "Corp SSO", groups: true }, { id: "other", display_name: "Other" }] }),
    });
    wrap(<GroupsView orgId={ORG} orgSlug="acme" />);
    fireEvent.click(await screen.findByRole("button", { name: "New group" }));
    const select = await screen.findByLabelText("Members");
    await waitFor(() => expect(within(select).getByRole("option", { name: "Linked to a sign-in provider" })).toBeInTheDocument());
    fireEvent.change(select, { target: { value: "idp" } });
    const provider = await screen.findByLabelText("Sign-in provider");
    expect(within(provider).getAllByRole("option").map((o) => o.textContent)).toEqual(["Corp SSO"]);
  });
});

describe("Group detail", () => {
  const detail = (g: ReturnType<typeof group>, members = [member]) => ({
    [`GET /v1/groups/${g.id}`]: () => json(200, g),
    [`GET /v1/groups/${g.id}/members`]: () => json(200, { items: members, next_cursor: null }),
    [`GET /v1/groups/${g.id}/workspaces`]: () =>
      json(200, { items: [{ workspace: { id: WS, name: "Main", slug: "main" }, role: "member", created_at: now }], next_cursor: null }),
    [`GET /v1/orgs/${ORG}/workspaces`]: () => json(200, { items: [{ id: WS, name: "Main", slug: "main" }] }),
    [`GET /v1/orgs/${ORG}/members`]: () => json(200, { items: [{ user_id: U, email: "ada@example.test", display_name: "Ada" }], next_cursor: null }),
    [`GET /v1/groups/${g.id}/impact`]: () =>
      json(200, {
        workspaces: [
          {
            workspace: { id: WS, name: "Main", slug: "main" },
            users_losing_access: [{ id: U, email: "ada@example.test", display_name: "Ada" }],
            total: 1,
            schedules_disabled: 2,
            webhooks_disabled: 0,
            api_keys: 1,
          },
        ],
      }),
  });

  it("a manual group can add and remove members; removal shows the impact first", async () => {
    const calls = stub({
      ...detail(group(G1, "manual")),
      [`POST /v1/groups/${G1}/members:batch`]: () => json(200, { added: [], removed: [U] }),
    });
    wrap(<GroupDetail orgId={ORG} orgSlug="acme" id={G1} />);
    expect(await screen.findByRole("form", { name: "Add member" })).toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Remove ada@example.test" }));
    expect(await screen.findByText(/1 person would lose all access to Main/)).toBeInTheDocument();
    expect(screen.getByText(/2 schedules and 0 webhooks would be disabled/)).toBeInTheDocument();
    expect(calls.some((c) => c.includes(`/impact?remove_member=${U}`))).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Remove member" }));
    await waitFor(() => expect(calls.some((c) => c.startsWith(`POST /v1/groups/${G1}/members:batch`))).toBe(true));
    expect(calls.find((c) => c.startsWith("POST"))).toContain(`"remove":["${U}"]`);
  });

  it("a sign-in-linked group is read-only and shows the claim value and last sync", async () => {
    stub(detail(group(G2, "idp"), [{ ...member, source: "idp", synced_at: now }]));
    wrap(<GroupDetail orgId={ORG} orgSlug="acme" id={G2} />);
    expect(await screen.findByText(/Members come from your sign-in provider/)).toBeInTheDocument();
    expect(screen.getByText("aurora-platform")).toBeInTheDocument();
    expect(screen.getByText("Last sync")).toBeInTheDocument();
    expect(screen.queryByRole("form", { name: "Add member" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Remove ada/ })).toBeNull();
  });

  it("a SCIM group is read-only with the directory note and a fixed name", async () => {
    stub(detail(group(G1, "scim")));
    wrap(<GroupDetail orgId={ORG} orgSlug="acme" id={G1} />);
    expect(await screen.findByText("Managed by your identity provider.")).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toBeDisabled();
    expect(screen.queryByRole("form", { name: "Add member" })).toBeNull();
  });

  it("Admin needs a confirmation (never Owner is offered)", async () => {
    const calls = stub({
      ...detail(group(G1, "manual")),
      [`PATCH /v1/groups/${G1}`]: () => json(200, group(G1, "manual", { org_role: "admin", revision: 2 })),
    });
    wrap(<GroupDetail orgId={ORG} orgSlug="acme" id={G1} />);
    const select = await screen.findByLabelText("Members act as");
    expect(within(select).queryByRole("option", { name: /owner/i })).toBeNull();
    fireEvent.change(select, { target: { value: "admin" } });
    expect(await screen.findByText("Make this group Admin?")).toBeInTheDocument();
    expect(calls.some((c) => c.startsWith("PATCH"))).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Make Admin" }));
    await waitFor(() => expect(calls.find((c) => c.startsWith("PATCH"))).toContain('"org_role":"admin"'));
  });
});

describe("Workspace Access with groups", () => {
  const access = {
    items: [{ user_id: U, email: "ada@example.test", display_name: "Ada", role: "owner", implicit: false, idp_value: null }],
    next_cursor: null,
    group_bindings: [{ group: { id: G1, name: "Platform team", source: "manual", member_count: 3 }, role: "approver" }],
    derived_members: {
      items: [
        {
          user: { id: "u2", email: "bob@example.test", display_name: "Bob" },
          role: "approver",
          sources: [{ kind: "group", group: { id: G1, name: "Platform team" } }],
        },
        {
          user: { id: "u3", email: "cy@example.test", display_name: "Cy" },
          role: "owner",
          sources: [{ kind: "org_role_via_group", group: { id: G2, name: "Admins" } }, { kind: "org_role" }],
        },
      ],
      next_cursor: null,
    },
  };

  it("lists bound groups and the people with access, one source chip per source", async () => {
    const calls = stub({
      [`GET /v1/workspaces/${WS}/access`]: () => json(200, access),
      [`GET /v1/orgs/${ORG}/members`]: () => json(200, { items: [], next_cursor: null }),
      [`GET /v1/orgs/${ORG}/groups`]: () => json(200, { items: [group(G1, "manual"), group(G2, "manual", { name: "Admins" })], next_cursor: null }),
    });
    wrap(<AccessTab wsId={WS} orgId={ORG} canOwn />);
    expect(await screen.findByText("Members with access")).toBeInTheDocument();
    expect(calls.some((c) => c.includes("expand=groups"))).toBe(true);
    expect(screen.getByLabelText("Role for group Platform team")).toHaveValue("approver");
    expect(screen.getByText("Group Platform team")).toBeInTheDocument();
    expect(screen.getByText("Org role via Admins")).toBeInTheDocument();
    expect(screen.getByText("Org admin/owner")).toBeInTheDocument();
    // only the unbound group is offered
    const pick = await screen.findByLabelText("Group");
    expect(within(pick).getAllByRole("option").map((o) => o.textContent)).toEqual(["Select a group…", "Admins (1)"]);
  });
});

describe("Policy identity picker", () => {
  const rule = (id: string) => ({
    name: "r",
    enabled: true,
    action: "block" as const,
    identities: [{ kind: "group" as const, id }],
    targets: [],
    conditions: [],
  });
  const render1 = (id: string, groups: { id: string; label: string }[] | undefined) =>
    wrap(
      <RuleEditorDialog
        open
        initial={rule(id) as never}
        data={{ agents: [], users: [], secrets: [], groups }}
        onClose={() => {}}
        onSave={() => {}}
      />,
    );

  it("offers a Group identity with the speaker explanation", async () => {
    render1(G1, [{ id: G1, label: "Platform team (3)" }]);
    expect(await screen.findByText(/member of the group/)).toBeInTheDocument();
    expect(screen.queryByText(/matches nobody/)).toBeNull();
  });

  it("shows a danger chip for a removed group, never a blank", async () => {
    render1(G2, [{ id: G1, label: "Platform team (3)" }]);
    expect(await screen.findByText("Group no longer exists — matches nobody")).toBeInTheDocument();
  });

  it("does not claim a group is gone when the caller cannot list groups", async () => {
    render1(G2, undefined);
    await screen.findByLabelText("Identity kind");
    expect(screen.queryByText(/no longer exists/)).toBeNull();
  });
});
