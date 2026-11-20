import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { historySentence } from "@/components/internet/internet-level";
import { expiresIn, peopleLine } from "@/components/org/members-view";
import { agentRowState, groupAgents, groupTitle, type OrgAgentRow } from "@/components/org/org-agents-list";
import { OrgRoleHelp } from "@/components/org/role-help";
import { pickRailWorkspace } from "@/lib/nav/manage-return";
import { ROLE_CAN } from "@/lib/roles-explain";

// P5-REFINE-17: the Manage area, the organization's agents list, invitations and role words.

const ws = (id: string, name: string, personal_of: { id: string; name: string } | null = null) =>
  ({ id, slug: id, name, personal_of }) as OrgAgentRow["ws"];
const row = (id: string, w: OrgAgentRow["ws"], state = "ready") =>
  ({ agent: { id, name: id, handle: id, state } as OrgAgentRow["agent"], ws: w });

describe("org agents list (org-6)", () => {
  it("groups team workspaces first A to Z, then personal ones by owner name, with the mockup's headers", () => {
    const acme = ws("acme", "Acme Team");
    const research = ws("research", "Research");
    const bob = ws("bob", "Your workspace", { id: "u2", name: "Bob Builder" });
    const carol = ws("carol", "Your workspace", { id: "u3", name: "Carol Member" });
    const groups = groupAgents([row("d1", carol), row("d2", research), row("d3", bob), row("d4", acme)]);
    expect(groups.map((g) => g.title)).toEqual(["Acme Team", "Research", "Bob Builder (personal)", "Carol Member (personal)"]);
    expect(groupTitle(bob)).toBe("Bob Builder (personal)");
  });

  it("words the header's states (D-425, C-1090)", () => {
    const word = (state: string) => agentRowState({ state } as never).word;
    expect(["paused", "needs_key", "failed", "working", "starting", "asleep", "ready"].map(word)).toEqual([
      "Paused", "Needs a key", "Can't start", "Working", "Starting its computer…", "Asleep", "Ready",
    ]);
  });
});

describe("invitations and people (org-4, org-13)", () => {
  const now = Date.parse("2026-11-20T12:00:00Z");
  it("reads when an invitation stops working", () => {
    expect(expiresIn("2026-11-26T12:00:00Z", now)).toBe("in 6 days");
    expect(expiresIn("2026-11-21T12:00:00Z", now)).toBe("in 1 day");
    expect(expiresIn("2026-11-20T18:00:00Z", now)).toBe("in 6 h");
    expect(expiresIn("2026-11-19T12:00:00Z", now)).toBe("expired");
  });
  it("sums up people and waiting invitations for the Overview", () => {
    expect(peopleLine(3, 1)).toBe("3 people · 1 invitation waiting");
    expect(peopleLine(1, 0)).toBe("1 person");
  });
});

describe("the Manage area's rail workspace (rec 8)", () => {
  const memberships = [
    {
      org: { slug: "acme" },
      workspaces: [
        { slug: "team", name: "Team", personal: false },
        { slug: "bob", name: "Bob", personal: true },
      ],
    },
  ];
  it("keeps the remembered workspace of this organization", () => {
    expect(pickRailWorkspace("acme", { orgSlug: "acme", wsSlug: "team", wsName: "Team" }, memberships)).toEqual({ wsSlug: "team", wsName: "Team" });
  });
  it("else the personal workspace, else the first", () => {
    expect(pickRailWorkspace("acme", null, memberships)?.wsSlug).toBe("bob");
    expect(pickRailWorkspace("acme", { orgSlug: "other", wsSlug: "x", wsName: "X" }, [{ org: { slug: "acme" }, workspaces: [{ slug: "team", name: "Team" }] }])?.wsSlug).toBe("team");
    expect(pickRailWorkspace("nope", null, memberships)).toBeNull();
  });
});

describe("roles in words (org-9)", () => {
  it("an Admin can invite people, set up apps and chat platforms, and change internet and AI model settings", () => {
    expect(ROLE_CAN.org.admin.can).toEqual(["Invite and remove people", "Set up apps and chat platforms", "Change internet and AI model settings"]);
  });
  it("the fold lists the three roles", () => {
    render(<OrgRoleHelp open />);
    expect(screen.getByText("What can each role do?")).toBeInTheDocument();
    for (const r of ["Member", "Admin", "Owner"]) expect(screen.getByText(r)).toBeInTheDocument();
  });
});

describe("history sentence (org-14)", () => {
  it("names who published which version", () => {
    expect(historySentence({ number: 4, published_by: { display_name: "Alice Admin" } })).toBe("Alice Admin published version 4");
    expect(historySentence({ number: 2 })).toBe("Someone published version 2");
  });
});

import { fireEvent, waitFor } from "@testing-library/react";
import { vi } from "vitest";
import { MembersView } from "@/components/org/members-view";
import { OrgAgentsList } from "@/components/org/org-agents-list";
import { json } from "./support/api";
import { wrap } from "./support/render";

function stub(routes: Record<string, (req: Request) => Response>) {
  const calls: string[] = [];
  vi.stubGlobal("fetch", vi.fn(async (input: Request | string | URL) => {
    const req = input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"));
    const key = `${req.method} ${new URL(req.url, "http://localhost").pathname}`;
    calls.push(key);
    const h = routes[key];
    return h ? h(req) : json(200, { items: [], next_cursor: null });
  }));
  return calls;
}

describe("organization agents from the one route (C-1090, D-425)", () => {
  it("shows the header's state and last use", async () => {
    stub({
      "GET /v1/orgs/o1/agents": () =>
        json(200, { items: [
          { id: "a1", name: "Daneel", handle: "daneel", kind: "hosted", state: "needs_key", last_active_at: null, workspace: { id: "w1", slug: "acme", name: "Acme Team", personal_of: null } },
          { id: "a2", name: "Eli", handle: "eli", kind: "hosted", state: "working", last_active_at: new Date().toISOString(), workspace: { id: "w1", slug: "acme", name: "Acme Team", personal_of: null } },
        ], next_cursor: null }),
    });
    wrap(<OrgAgentsList orgId="o1" orgSlug="acme" />);
    expect(await screen.findByText("Needs a key")).toBeInTheDocument();
    expect(screen.getByText("Working")).toBeInTheDocument();
    expect(screen.getByText("never used")).toBeInTheDocument();
  });
});

describe("invitation links (org-4, C-1069)", () => {
  it("Copy re-issues the link and says so; Resend only when mail works", async () => {
    const written: string[] = [];
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t: string) => void written.push(t) }, configurable: true });
    const inv = { id: "i1", email: "dana@example.test", role: "member", workspace_ids: [], approver_workspace_ids: [], expires_at: new Date(Date.now() + 6 * 86400000).toISOString(), invited_by: { display_name: "Alice Admin" } };
    stub({
      "GET /v1/orgs/o1/invitations": () => json(200, { items: [inv], next_cursor: null }),
      "POST /v1/invitations/i1/link": () => json(200, { link: "https://a.test/join?token=t", expires_at: inv.expires_at }),
      "GET /v1/instance": () => json(200, { email_delivery: "log", features: {} }),
    });
    wrap(<MembersView orgId="o1" viewer={{ role: "owner" } as never} />);
    fireEvent.click(await screen.findByRole("button", { name: "Copy invitation link for dana@example.test" }));
    await waitFor(() => expect(written).toEqual(["https://a.test/join?token=t"]));
    expect(await screen.findByTestId("invite-copied")).toHaveTextContent("Invitation link copied. Send it to dana yourself.");
    expect(screen.queryByRole("button", { name: /^Resend/ })).toBeNull();
    expect(screen.getByText("E-mail isn't set up, so copy each invitation link and send it yourself.")).toBeInTheDocument();
  });
});
